use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, Method, StatusCode},
    routing::any,
};
use reqwest::{Client, Url};
use std::{future::Future, time::Duration};
use tanuki_client::{
    ClaimRelease, ExpiryUpdate, Session, StateTopic,
    protocol::{ClientName, NonNegativeDuration, Selector},
};
use tokio::{
    net::TcpListener,
    sync::mpsc,
    time::{Instant, sleep_until},
};

use roxmltree::Document;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Tanuki session ended: {0}")]
    Session(#[from] tanuki_client::SessionEnd),
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("URL: {0}")]
    Url(#[from] url::ParseError),
    #[error("Tanuki: {0}")]
    Tanuki(#[from] tanuki_client::ClientError),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("UTF-8: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("UTF-8 body: {0}")]
    StringUtf8(#[from] std::string::FromUtf8Error),
    #[error("invalid XML: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("invalid UPnP response: {0}")]
    Protocol(&'static str),
}

/// Only instance zero's Master channel describes this example's TV volume.
pub fn notification_volume(xml: &str) -> Result<Option<u8>, Error> {
    let outer = Document::parse(xml)?;
    let Some(change) = outer
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "LastChange")
    else {
        return Ok(None);
    };
    let inner = Document::parse(change.text().ok_or(Error::Protocol("empty LastChange"))?)?;
    for instance in inner.descendants().filter(|n| {
        n.is_element() && n.tag_name().name() == "InstanceID" && n.attribute("val") == Some("0")
    }) {
        if let Some(volume) = instance.children().find(|n| {
            n.is_element()
                && n.tag_name().name() == "Volume"
                && n.attribute("channel") == Some("Master")
        }) {
            return volume
                .attribute("val")
                .and_then(|s| s.parse::<u8>().ok())
                .filter(|v| *v <= 100)
                .map(Some)
                .ok_or(Error::Protocol("volume must be 0..100"));
        }
    }
    Ok(None)
}

#[derive(Clone, Debug)]
pub struct Config {
    pub description_url: String,
    pub callback_url: String,
    pub tanuki_url: String,
}

struct Service {
    control: Url,
    events: Url,
    kind: String,
}

fn http_url(value: &str) -> Result<Url, Error> {
    let url = Url::parse(value)?;
    if url.scheme() != "http"
        || url.host_str().is_none()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Error::Protocol(
            "UPnP URLs must be HTTP URLs without credentials or fragments",
        ));
    }
    Ok(url)
}

fn service(xml: &str, location: &Url) -> Result<Service, Error> {
    let doc = Document::parse(xml)?;
    let base = doc
        .descendants()
        .find(|n| n.has_tag_name("URLBase"))
        .and_then(|n| n.text())
        .map(http_url)
        .transpose()?
        .unwrap_or_else(|| location.clone());
    for node in doc.descendants().filter(|n| n.has_tag_name("service")) {
        let field = |name| {
            node.children()
                .find(|n| n.has_tag_name(name))
                .and_then(|n| n.text())
        };
        let Some(kind) = field("serviceType") else {
            continue;
        };
        if !matches!(
            kind,
            "urn:schemas-upnp-org:service:RenderingControl:1"
                | "urn:schemas-upnp-org:service:RenderingControl:2"
                | "urn:schemas-upnp-org:service:RenderingControl:3"
        ) {
            continue;
        }
        let resolve = |name| -> Result<Url, Error> {
            let joined = base.join(field(name).ok_or(Error::Protocol("missing service URL"))?)?;
            http_url(joined.as_str())
        };
        return Ok(Service {
            control: resolve("controlURL")?,
            events: resolve("eventSubURL")?,
            kind: kind.into(),
        });
    }
    Err(Error::Protocol("device has no RenderingControl service"))
}

async fn xml_body(mut response: reqwest::Response) -> Result<String, Error> {
    response.error_for_status_ref()?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > 64 * 1024 {
            return Err(Error::Protocol("UPnP response exceeds 64 KiB"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

struct Subscription {
    sid: String,
    renew_at: Instant,
}
fn subscription(headers: &HeaderMap) -> Result<Subscription, Error> {
    let sid = headers
        .get("sid")
        .and_then(|v| v.to_str().ok())
        .filter(|s| s.starts_with("uuid:"))
        .ok_or(Error::Protocol("missing subscription SID"))?
        .to_owned();
    let timeout = headers
        .get("timeout")
        .and_then(|v| v.to_str().ok())
        .ok_or(Error::Protocol("missing subscription timeout"))?;
    let seconds = if timeout.eq_ignore_ascii_case("Second-infinite") {
        60
    } else {
        timeout
            .strip_prefix("Second-")
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|s| *s >= 2)
            .ok_or(Error::Protocol("invalid subscription timeout"))?
    };
    Ok(Subscription {
        sid,
        renew_at: Instant::now() + Duration::from_secs((seconds / 2).min(30)),
    })
}

struct Notification {
    sid: String,
    seq: u32,
    volume: Option<u8>,
}
async fn notify(
    State(tx): State<mpsc::Sender<Notification>>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if method.as_str() != "NOTIFY" {
        return StatusCode::METHOD_NOT_ALLOWED;
    }
    let header = |name| headers.get(name).and_then(|v| v.to_str().ok());
    if header("nt") != Some("upnp:event") || header("nts") != Some("upnp:propchange") {
        return StatusCode::BAD_REQUEST;
    }
    let (Some(sid), Some(seq)) = (
        header("sid"),
        header("seq").and_then(|v| v.parse::<u32>().ok()),
    ) else {
        return StatusCode::BAD_REQUEST;
    };
    let volume = match std::str::from_utf8(&body)
        .map_err(Error::Utf8)
        .and_then(notification_volume)
    {
        Ok(v) => v,
        Err(error) => {
            tracing::warn!(%error, "invalid TV notification");
            return StatusCode::BAD_REQUEST;
        }
    };
    match tx.try_send(Notification {
        sid: sid.into(),
        seq,
        volume,
    }) {
        Ok(()) => StatusCode::OK,
        Err(error) => {
            tracing::warn!(%error, "notification queue unavailable");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

async fn publish(actual: &StateTopic<u8>, volume: u8) -> Result<(), Error> {
    let lease = NonNegativeDuration::new(
        std::time::Duration::from_secs(90)
            .try_into()
            .expect("90 seconds fit"),
    )
    .expect("positive lease");
    let receipt = actual.publish(&volume, ExpiryUpdate::Set(lease)).await?;
    for warning in receipt.warnings {
        tracing::warn!(?warning, "Tanuki publication warning");
    }
    Ok(())
}

async fn set_volume(client: &Client, service: &Service, volume: u8) -> Result<(), Error> {
    let body = format!(
        "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:SetVolume xmlns:u=\"{}\"><InstanceID>0</InstanceID><Channel>Master</Channel><DesiredVolume>{volume}</DesiredVolume></u:SetVolume></s:Body></s:Envelope>",
        service.kind
    );
    let response = client
        .post(service.control.clone())
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header("SOAPACTION", format!("\"{}#SetVolume\"", service.kind))
        .body(body)
        .send()
        .await?;
    let xml = xml_body(response).await?;
    let doc = Document::parse(&xml)?;
    if !doc
        .descendants()
        .any(|n| n.is_element() && n.tag_name().name() == "SetVolumeResponse")
        || doc
            .descendants()
            .any(|n| n.is_element() && n.tag_name().name() == "Fault")
    {
        return Err(Error::Protocol(
            "SetVolume did not return a successful SOAP response",
        ));
    }
    Ok(())
}

/// One connection lifetime. On failure a supervisor can restart with a fresh
/// subscription and Tanuki session. Never replays an uncertain SOAP call here.
pub async fn run(
    config: Config,
    listener: TcpListener,
    stop: impl Future<Output = ()>,
) -> Result<(), Error> {
    let callback = http_url(&config.callback_url)?;
    if callback.path() != "/notify" || callback.query().is_some() {
        return Err(Error::Protocol(
            "callback URL must use /notify without a query",
        ));
    }
    let location = http_url(&config.description_url)?;
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()?;
    let service = service(
        &xml_body(client.get(location.clone()).send().await?).await?,
        &location,
    )?;
    let session = Session::connect(
        &config.tanuki_url,
        ClientName::parse("tv-volume bridge").expect("constant name"),
        vec![Selector::parse("/tv/desired-volume").expect("constant selector")],
    )
    .await?;
    let actual = session.state::<u8>("/tv/volume")?;
    let (tx, mut rx) = mpsc::channel(64);
    let app = Router::new()
        .route(callback.path(), any(notify))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(tx);
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let mut active: Option<Subscription> = None;
    let work = async {
        // Remove a previous process's cached state before subscribing.
        actual.remove().await?;
        let desired = session.desired::<u8>("/tv/desired-volume")?;
        let mut setup = session.batch();
        setup
            .define(&desired)?
            .claim(&desired, ClaimRelease::Immediate)?;
        let setup_receipt = setup.commit().await?;
        for warning in setup_receipt.warnings {
            tracing::warn!(?warning, "Tanuki setup warning");
        }
        let mut intent = session.observe(&desired).await?;
        let response = client
            .request(
                Method::from_bytes(b"SUBSCRIBE").expect("constant method"),
                service.events.clone(),
            )
            .header("CALLBACK", format!("<{callback}>"))
            .header("NT", "upnp:event")
            .header("TIMEOUT", "Second-60")
            .send()
            .await?
            .error_for_status()?;
        active = Some(subscription(response.headers())?);
        tracing::info!(events = %service.events, "TV event subscription active");
        let mut volume = None;
        let mut last_seq = None;
        let mut last_intent = None;
        let mut claim_id = None;
        let initial_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let sub = active.as_ref().expect("subscribed");
            tokio::select! {
                message = rx.recv() => {
                    let message = message.ok_or(Error::Protocol("callback server stopped"))?;
                    if message.sid != sub.sid { tracing::warn!(sid = %message.sid, "ignoring notification for another subscription"); continue; }
                    if last_seq == Some(message.seq) { continue; }
                    let expected = last_seq.map(|n: u32| if n == u32::MAX { 1 } else { n + 1 }).unwrap_or(0);
                    if message.seq != expected { return Err(Error::Protocol("notification sequence gap; resubscribe to recover state")); }
                    last_seq = Some(message.seq);
                    if let Some(value) = message.volume { publish(&actual, value).await?; volume = Some(value); }
                }
                snapshot = intent.recv() => {
                    let snapshot = snapshot?.ok_or(Error::Protocol("Tanuki observer closed"))?;
                    // A write receipt can arrive before the SDK processes its update.
                    // Wait for an observed snapshot that includes our definition/claim.
                    if snapshot.sequence() < setup_receipt.sequence { continue; }
                    match snapshot.read(&desired) {
                        Ok(Some(node)) => {
                            let tanuki_client::DecodedNode::Desired { claim: Some(claim), .. } = &node else {
                                return Err(Error::Protocol("desired volume claim was released"));
                            };
                            if claim.owner != "tv-volume bridge" || claim_id.is_some_and(|id| id != claim.id) {
                                return Err(Error::Protocol("desired volume claim was taken by another controller"));
                            }
                            claim_id = Some(claim.id);
                            if let Some(current) = node.current() {
                            if last_intent.as_ref() == Some(&current.last_write) { continue; }
                            last_intent = Some(current.last_write.clone());
                            if current.value > 100 { tracing::warn!(value = current.value, "desired volume outside 0..100"); continue; }
                            if let Err(error) = set_volume(&client, &service, current.value).await { tracing::warn!(%error, "SetVolume failed; not retrying this request"); }
                            }
                        },
                        Ok(None) => return Err(Error::Protocol("desired volume definition removed")),
                        Err(error) => tracing::warn!(%error, "invalid desired volume"),
                    }
                }
                _ = sleep_until(sub.renew_at) => {
                    let response = client.request(Method::from_bytes(b"SUBSCRIBE").expect("constant method"), service.events.clone()).header("SID", &sub.sid).header("TIMEOUT", "Second-60").send().await?.error_for_status()?;
                    let renewed = subscription(response.headers())?;
                    if renewed.sid != sub.sid { return Err(Error::Protocol("renewal changed subscription SID")); }
                    active = Some(renewed);
                    if let Some(value) = volume { publish(&actual, value).await?; }
                }
                _ = sleep_until(initial_deadline), if volume.is_none() => return Err(Error::Protocol("TV sent no initial Master volume within 10 seconds")),
            }
        }
    };
    let result = tokio::select! { result = work => result, () = stop => Ok(()) };
    if let Err(error) = &result {
        tracing::warn!(%error, "bridge stopped");
    }
    if let Some(sub) = active {
        match client
            .request(
                Method::from_bytes(b"UNSUBSCRIBE").expect("constant method"),
                service.events,
            )
            .header("SID", sub.sid)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
        {
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "unsubscribe failed"),
        }
    }
    server.abort();
    let _ = server.await;
    if let Err(error) = actual.remove().await {
        tracing::warn!(%error, "could not remove observed volume; expiry will clear it");
    }
    if let Err(error) = session.close().await {
        tracing::warn!(%error, "Tanuki session close failed");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(inner: &str) -> String {
        format!(
            "<e:propertyset xmlns:e='urn:schemas-upnp-org:event-1-0'><e:property><LastChange><![CDATA[<Event xmlns='urn:schemas-upnp-org:metadata-1-0/RCS/'>{inner}</Event>]]></LastChange></e:property></e:propertyset>"
        )
    }

    #[test]
    fn master_volume_is_selected_from_instance_zero() {
        let xml = event(
            "<InstanceID val='1'><Volume channel='Master' val='99'/></InstanceID><InstanceID val='0'><Volume channel='LF' val='80'/><Volume channel='Master' val='25'/></InstanceID>",
        );
        assert_eq!(notification_volume(&xml).unwrap(), Some(25));
    }

    #[test]
    fn mute_only_is_not_a_volume_change_and_invalid_volume_is_rejected() {
        assert_eq!(
            notification_volume(&event(
                "<InstanceID val='0'><Mute channel='Master' val='1'/></InstanceID>"
            ))
            .unwrap(),
            None
        );
        assert!(
            notification_volume(&event(
                "<InstanceID val='0'><Volume channel='Master' val='101'/></InstanceID>"
            ))
            .is_err()
        );
        assert!(notification_volume("not xml").is_err());
    }
    #[test]
    fn discovery_resolves_relative_urls_and_url_base_in_namespaced_descriptions() {
        let location = http_url("http://tv:9197/device/description.xml").unwrap();
        let xml = "<root xmlns='urn:schemas-upnp-org:device-1-0'><URLBase>http://tv:7676/base/</URLBase><device><serviceList><service><serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType><controlURL>control</controlURL><eventSubURL>/events</eventSubURL></service></serviceList></device></root>";
        let found = service(xml, &location).unwrap();
        assert_eq!(found.control.as_str(), "http://tv:7676/base/control");
        assert_eq!(found.events.as_str(), "http://tv:7676/events");
        let xml = xml.replace("<URLBase>http://tv:7676/base/</URLBase>", "");
        assert_eq!(
            service(&xml, &location).unwrap().control.as_str(),
            "http://tv:9197/device/control"
        );
        assert!(service("<root/>", &location).is_err());
    }

    #[tokio::test]
    async fn subscription_uses_granted_timeout_and_rejects_missing_headers() {
        let mut headers = HeaderMap::new();
        assert!(subscription(&headers).is_err());
        headers.insert("sid", "uuid:example".parse().unwrap());
        headers.insert("timeout", "Second-4".parse().unwrap());
        let before = Instant::now();
        let sub = subscription(&headers).unwrap();
        assert!(sub.renew_at >= before + Duration::from_secs(2));
        assert!(sub.renew_at <= Instant::now() + Duration::from_secs(2));
        headers.insert("timeout", "Second-0".parse().unwrap());
        assert!(subscription(&headers).is_err());
    }
}

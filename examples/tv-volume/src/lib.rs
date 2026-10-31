pub mod discovery;

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
    #[error("discovery description task failed: {0}")]
    DiscoveryTask(#[source] tokio::task::JoinError),
    #[error(transparent)]
    Selection(#[from] discovery::SelectionError),
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

/// The changed fields of instance zero's Master audio channel.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderingChange {
    volume: Option<u8>,
    mute: Option<bool>,
}
impl RenderingChange {
    pub fn volume(&self) -> Option<u8> {
        self.volume
    }
    pub fn mute(&self) -> Option<bool> {
        self.mute
    }
    fn merge(&mut self, newer: Self) {
        if newer.volume.is_some() {
            self.volume = newer.volume;
        }
        if newer.mute.is_some() {
            self.mute = newer.mute;
        }
    }
}

/// Parse only reported fields; missing mute never implies an unmuted TV.
pub fn notification_change(xml: &str) -> Result<RenderingChange, Error> {
    let outer = Document::parse(xml)?;
    let Some(change) = outer.descendants().find(|n| n.has_tag_name("LastChange")) else {
        return Ok(RenderingChange::default());
    };
    let text = change.text().unwrap_or_default().trim();
    if text.is_empty() {
        // Samsung also sends empty events, whose SEQ must still be consumed.
        tracing::debug!("empty LastChange notification has no audio update");
        return Ok(RenderingChange::default());
    }
    let inner = Document::parse(text)?;
    let mut change = RenderingChange::default();
    for instance in inner
        .descendants()
        .filter(|n| n.has_tag_name("InstanceID") && n.attribute("val") == Some("0"))
    {
        for field in instance
            .children()
            .filter(|n| n.is_element() && n.attribute("channel") == Some("Master"))
        {
            match field.tag_name().name() {
                "Volume" => {
                    change.volume = Some(
                        field
                            .attribute("val")
                            .and_then(|s| s.parse::<u8>().ok())
                            .filter(|v| *v <= 100)
                            .ok_or(Error::Protocol("volume must be 0..100"))?,
                    )
                }
                "Mute" => {
                    change.mute = Some(match field.attribute("val") {
                        Some("0" | "false" | "no") => false,
                        Some("1" | "true" | "yes") => true,
                        _ => return Err(Error::Protocol("mute must be a UPnP boolean")),
                    })
                }
                _ => {}
            }
        }
    }
    Ok(change)
}

/// Compatibility helper for consumers interested only in volume.
pub fn notification_volume(xml: &str) -> Result<Option<u8>, Error> {
    Ok(notification_change(xml)?.volume)
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
    change: RenderingChange,
}
struct NotificationSequence {
    next: u32,
    pending: std::collections::BTreeMap<u32, RenderingChange>,
    gap_deadline: Option<Instant>,
}
impl NotificationSequence {
    fn new() -> Self {
        Self {
            next: 0,
            pending: Default::default(),
            gap_deadline: None,
        }
    }
    fn push(
        &mut self,
        seq: u32,
        change: RenderingChange,
        now: Instant,
    ) -> Result<RenderingChange, Error> {
        // Sequence zero is reserved for the initial event; wrapped streams
        // continue at one. Ignore duplicates/late packets behind our frontier.
        if seq != self.next && (seq == 0 || seq.wrapping_sub(self.next) >= (1 << 31)) {
            return Ok(RenderingChange::default());
        }
        let previous_next = self.next;
        self.pending.entry(seq).or_insert(change);
        let mut latest = RenderingChange::default();
        while let Some(change) = self.pending.remove(&self.next) {
            latest.merge(change);
            self.next = self.next.wrapping_add(1).max(1);
        }
        if self.pending.len() > 64 {
            return Err(Error::Protocol(
                "notification reorder buffer full; resubscribe to recover state",
            ));
        }
        if self.pending.is_empty() {
            self.gap_deadline = None;
        } else if self.next != previous_next || self.gap_deadline.is_none() {
            self.gap_deadline = Some(now + Duration::from_secs(1));
        }
        Ok(latest)
    }
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
    let change = match std::str::from_utf8(&body)
        .map_err(Error::Utf8)
        .and_then(notification_change)
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
        change,
    }) {
        Ok(()) => StatusCode::OK,
        Err(error) => {
            tracing::warn!(%error, "notification queue unavailable");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

async fn publish(
    session: &Session,
    actual: &StateTopic<u8>,
    mute: &StateTopic<bool>,
    change: RenderingChange,
) -> Result<(), Error> {
    if change == RenderingChange::default() {
        return Ok(());
    }
    let lease = NonNegativeDuration::new(
        std::time::Duration::from_secs(90)
            .try_into()
            .expect("90 seconds fit"),
    )
    .expect("positive lease");
    let mut batch = session.batch();
    if let Some(value) = change.volume {
        batch.publish(actual, &value, ExpiryUpdate::Set(lease))?;
    }
    if let Some(value) = change.mute {
        batch.publish(mute, &value, ExpiryUpdate::Set(lease))?;
    }
    for warning in batch.commit().await?.warnings {
        tracing::warn!(?warning, "Tanuki publication warning");
    }
    Ok(())
}

enum Control {
    Volume(u8),
    Mute(bool),
}
async fn set_audio(client: &Client, service: &Service, control: Control) -> Result<(), Error> {
    let (action, argument, value) = match control {
        Control::Volume(value) => ("SetVolume", "DesiredVolume", value.to_string()),
        Control::Mute(value) => ("SetMute", "DesiredMute", u8::from(value).to_string()),
    };
    let body = format!(
        "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{}\"><InstanceID>0</InstanceID><Channel>Master</Channel><{argument}>{value}</{argument}></u:{action}></s:Body></s:Envelope>",
        service.kind
    );
    let response = client
        .post(service.control.clone())
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header("SOAPACTION", format!("\"{}#{action}\"", service.kind))
        .body(body)
        .send()
        .await?;
    let xml = xml_body(response).await?;
    let doc = Document::parse(&xml)?;
    let expected = format!("{action}Response");
    if !doc.descendants().any(|n| n.has_tag_name(expected.as_str()))
        || doc.descendants().any(|n| n.has_tag_name("Fault"))
    {
        return Err(Error::Protocol(
            "audio action did not return a successful SOAP response",
        ));
    }
    Ok(())
}

/// The two typed inputs share claim checks and per-write deduplication.
#[derive(Default)]
struct InputTracker {
    claim_id: Option<u64>,
    last_write: Option<tanuki_client::protocol::ProvenanceView>,
}
impl InputTracker {
    fn accept<T>(
        &mut self,
        raw: Option<&tanuki_client::protocol::NodeView>,
        decoded: Result<Option<tanuki_client::DecodedNode<T>>, tanuki_client::ReadError>,
    ) -> Result<Option<T>, Error> {
        // Claim authority is independent of whether the pending payload decodes.
        let claim = match raw {
            Some(tanuki_client::protocol::NodeView::Desired {
                claim: Some(claim), ..
            }) => claim,
            Some(_) => return Err(Error::Protocol("desired audio input claim was released")),
            None => return Err(Error::Protocol("desired audio input definition removed")),
        };
        if claim.owner != "tv-volume bridge" || self.claim_id.is_some_and(|id| id != claim.id) {
            return Err(Error::Protocol(
                "desired audio input claim was taken by another controller",
            ));
        }
        self.claim_id = Some(claim.id);
        let current = match decoded {
            Ok(Some(tanuki_client::DecodedNode::Desired { current, .. })) => current,
            Ok(_) => return Err(Error::Protocol("desired audio input changed kind")),
            Err(error) => {
                tracing::warn!(%error, "invalid desired audio value");
                return Ok(None);
            }
        };
        let Some(current) = current else {
            return Ok(None);
        };
        if self.last_write.as_ref() == Some(&current.last_write) {
            return Ok(None);
        }
        self.last_write = Some(current.last_write);
        Ok(Some(current.value))
    }
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
        vec![Selector::parse("/tv/{desired-volume,desired-mute}").expect("constant selector")],
    )
    .await?;
    let actual = session.state::<u8>("/tv/volume")?;
    let mute = session.state::<bool>("/tv/mute")?;
    let (tx, mut rx) = mpsc::channel(64);
    let app = Router::new()
        .route(callback.path(), any(notify))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(tx);
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let mut active: Option<Subscription> = None;
    let work = async {
        // Remove a previous process's cached state before subscribing.
        let mut clear = session.batch();
        clear.remove(&actual)?.remove(&mute)?;
        clear.commit().await?;
        let desired = session.desired::<u8>("/tv/desired-volume")?;
        let desired_mute = session.desired::<bool>("/tv/desired-mute")?;
        let mut setup = session.batch();
        setup
            .define(&desired)?
            .claim(&desired, ClaimRelease::Immediate)?
            .define(&desired_mute)?
            .claim(&desired_mute, ClaimRelease::Immediate)?;
        let setup_receipt = setup.commit().await?;
        for warning in setup_receipt.warnings {
            tracing::warn!(?warning, "Tanuki setup warning");
        }
        let mut intent = session
            .observe_batch([desired.path().clone(), desired_mute.path().clone()])
            .await?;
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
        let mut observed = RenderingChange::default();
        let mut sequence = NotificationSequence::new();
        let mut volume_input = InputTracker::default();
        let mut mute_input = InputTracker::default();
        let initial_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let sub = active.as_ref().expect("subscribed");
            tokio::select! {
                message = rx.recv() => {
                    let message = message.ok_or(Error::Protocol("callback server stopped"))?;
                    if message.sid != sub.sid { tracing::warn!(sid = %message.sid, "ignoring notification for another subscription"); continue; }
                    let change = sequence.push(message.seq, message.change, Instant::now())?;
                    publish(&session, &actual, &mute, change).await?;
                    observed.merge(change);
                }
                snapshot = intent.recv() => {
                    let snapshot = snapshot?.ok_or(Error::Protocol("Tanuki observer closed"))?;
                    // A write receipt can arrive before the SDK processes its update.
                    // Wait for an observed snapshot that includes our definition/claim.
                    if snapshot.sequence() < setup_receipt.sequence { continue; }
                    let volume_request = volume_input.accept(snapshot.node(desired.path()), snapshot.read(&desired))?;
                    let mute_request = mute_input.accept(snapshot.node(desired_mute.path()), snapshot.read(&desired_mute))?;
                    if let Some(value) = volume_request {
                        if value <= 100 {
                            if let Err(error) = set_audio(&client, &service, Control::Volume(value)).await { tracing::warn!(%error, "SetVolume failed; not retrying this request"); }
                        } else { tracing::warn!(value, "desired volume outside 0..100"); }
                    }
                    if let Some(value) = mute_request
                        && let Err(error) = set_audio(&client, &service, Control::Mute(value)).await
                    { tracing::warn!(%error, "SetMute failed; not retrying this request"); }
                }
                _ = sleep_until(sub.renew_at) => {
                    let response = client.request(Method::from_bytes(b"SUBSCRIBE").expect("constant method"), service.events.clone()).header("SID", &sub.sid).header("TIMEOUT", "Second-60").send().await?.error_for_status()?;
                    let renewed = subscription(response.headers())?;
                    if renewed.sid != sub.sid { return Err(Error::Protocol("renewal changed subscription SID")); }
                    active = Some(renewed);
                    publish(&session, &actual, &mute, observed).await?;
                }
                _ = sleep_until(sequence.gap_deadline.unwrap_or(initial_deadline)), if sequence.gap_deadline.is_some() => {
                    return Err(Error::Protocol("notification sequence gap persisted for one second; resubscribe to recover state"));
                }
                _ = sleep_until(initial_deadline), if observed.volume.is_none() => return Err(Error::Protocol("TV sent no initial Master volume within 10 seconds")),
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
    let mut clear = session.batch();
    clear.remove(&actual)?.remove(&mute)?;
    if let Err(error) = clear.commit().await {
        tracing::warn!(%error, "could not remove observed audio; expiry will clear it");
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
    fn notifications_expose_master_mute_with_volume_or_on_its_own() {
        let change = notification_change(&event("<InstanceID val='1'><Mute channel='Master' val='1'/></InstanceID><InstanceID val='0'><Mute channel='LF' val='1'/><Volume channel='Master' val='25'/><Mute channel='Master' val='0'/></InstanceID>")).unwrap();
        assert_eq!(change.volume(), Some(25));
        assert_eq!(change.mute(), Some(false));
        for (text, expected) in [
            ("1", true),
            ("0", false),
            ("true", true),
            ("false", false),
            ("yes", true),
            ("no", false),
        ] {
            let change = notification_change(&event(&format!(
                "<InstanceID val='0'><Mute channel='Master' val='{text}'/></InstanceID>"
            )))
            .unwrap();
            assert_eq!(change.volume(), None);
            assert_eq!(change.mute(), Some(expected));
        }
        assert!(
            notification_change(&event(
                "<InstanceID val='0'><Mute channel='Master' val='2'/></InstanceID>"
            ))
            .is_err()
        );
    }

    #[test]
    fn reordered_notifications_preserve_volume_order_and_duplicates_do_not_roll_back() {
        let now = Instant::now();
        let change = |volume| RenderingChange { volume, mute: None };
        let mut sequence = NotificationSequence::new();
        assert_eq!(
            sequence.push(0, change(Some(25)), now).unwrap(),
            change(Some(25))
        );
        assert_eq!(
            sequence.push(2, change(Some(40)), now).unwrap(),
            change(None)
        );
        assert!(sequence.gap_deadline.is_some());
        assert_eq!(
            sequence.push(1, change(Some(30)), now).unwrap(),
            change(Some(40))
        );
        assert!(sequence.gap_deadline.is_none());
        assert_eq!(
            sequence.push(1, change(Some(99)), now).unwrap(),
            change(None)
        );
        assert_eq!(sequence.push(4, change(None), now).unwrap(), change(None));
        assert_eq!(
            sequence.push(3, change(Some(39)), now).unwrap(),
            change(Some(39))
        );
        assert!(sequence.gap_deadline.is_none());
    }

    #[test]
    fn missing_events_have_a_bounded_wait_and_reorder_capacity() {
        let now = Instant::now();
        let change = |volume| RenderingChange { volume, mute: None };
        let mut sequence = NotificationSequence::new();
        sequence.push(0, change(None), now).unwrap();
        for seq in 2..=65 {
            assert_eq!(sequence.push(seq, change(None), now).unwrap(), change(None));
        }
        assert_eq!(sequence.gap_deadline, Some(now + Duration::from_secs(1)));
        assert!(sequence.push(66, change(None), now).is_err());
        let mut sequence = NotificationSequence::new();
        sequence.next = u32::MAX;
        assert_eq!(
            sequence.push(1, change(Some(11)), now).unwrap(),
            change(None)
        );
        assert_eq!(
            sequence.push(u32::MAX, change(Some(10)), now).unwrap(),
            change(Some(11))
        );
        assert_eq!(sequence.next, 2);
        assert_eq!(
            sequence.push(u32::MAX, change(Some(99)), now).unwrap(),
            change(None)
        );
    }

    #[test]
    fn empty_last_change_is_a_no_op_including_whitespace_and_empty_cdata() {
        for change in [
            "<LastChange/>",
            "<LastChange></LastChange>",
            "<LastChange> \n\t </LastChange>",
            "<LastChange><![CDATA[]]></LastChange>",
            "<LastChange><![CDATA[ \n ]]></LastChange>",
        ] {
            let xml = format!(
                "<e:propertyset xmlns:e='urn:schemas-upnp-org:event-1-0'><e:property>{change}</e:property></e:propertyset>"
            );
            assert_eq!(notification_volume(&xml).unwrap(), None, "{change}");
        }
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

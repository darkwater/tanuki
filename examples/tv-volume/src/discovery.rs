//! Read-only SSDP discovery: UDP searches and HTTP description GETs only.
use crate::Error;
use reqwest::Url;
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};
use tokio::{
    net::{UdpSocket, lookup_host},
    task::JoinSet,
    time::{Instant, timeout_at},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredDevice {
    udn: String,
    name: String,
    description_url: Url,
}
impl DiscoveredDevice {
    pub fn udn(&self) -> &str {
        &self.udn
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn description_url(&self) -> &Url {
        &self.description_url
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SelectionError {
    #[error("no RenderingControl devices found; check the TV and multicast interface")]
    NoneFound,
    #[error("no discovered device matches {0:?}; use --discover to list devices")]
    NoMatch(String),
    #[error("multiple devices match; choose --device with a UDN from --discover: {0:?}")]
    Ambiguous(Vec<String>),
}

pub fn select_device<'a>(
    devices: &'a [DiscoveredDevice],
    selector: Option<&str>,
) -> Result<&'a DiscoveredDevice, SelectionError> {
    if devices.is_empty() {
        return Err(SelectionError::NoneFound);
    }
    if let Some(selector) = selector
        && let Some(device) = devices.iter().find(|d| d.udn == selector)
    {
        return Ok(device);
    }
    let matches: Vec<_> = devices
        .iter()
        .filter(|d| selector.is_none_or(|s| d.name == s))
        .collect();
    match matches.as_slice() {
        [device] => Ok(device),
        [] => Err(SelectionError::NoMatch(selector.unwrap_or_default().into())),
        _ => Err(SelectionError::Ambiguous(
            matches
                .iter()
                .map(|d| format!("{} ({})", d.name, d.udn))
                .collect(),
        )),
    }
}

fn response_location(packet: &[u8]) -> Option<Url> {
    let text = std::str::from_utf8(packet).ok()?;
    let mut lines = text.lines();
    let mut status = lines.next()?.split_whitespace();
    if status.next()? != "HTTP/1.1" || status.next()? != "200" {
        return None;
    }
    for line in lines.take_while(|line| !line.is_empty()) {
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("location")
        {
            return crate::http_url(value.trim()).ok();
        }
    }
    None
}

fn device_description(xml: &str, location: Url) -> Result<DiscoveredDevice, Error> {
    crate::service(xml, &location)?;
    let doc = roxmltree::Document::parse(xml)?;
    let device = doc
        .root_element()
        .children()
        .find(|n| n.has_tag_name("device"))
        .ok_or(Error::Protocol("missing device description"))?;
    let field = |name| {
        device
            .children()
            .find(|n| n.has_tag_name(name))
            .and_then(|n| n.text())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let udn = field("UDN")
        .filter(|s| s.starts_with("uuid:"))
        .ok_or(Error::Protocol("missing device UDN"))?
        .to_owned();
    let name = field("friendlyName").unwrap_or(&udn).to_owned();
    Ok(DiscoveredDevice {
        udn,
        name,
        description_url: location,
    })
}

/// Searches the default IPv4 multicast route. Only UDP M-SEARCH and HTTP GET
/// are used; this never opens a Tanuki session or sends device control actions.
pub async fn discover() -> Result<Vec<DiscoveredDevice>, Error> {
    discover_at(
        "239.255.255.250:1900".parse().expect("constant address"),
        Ipv4Addr::UNSPECIFIED,
        Duration::from_secs(4),
    )
    .await
}

async fn fetch_device(client: reqwest::Client, location: Url) -> Result<DiscoveredDevice, Error> {
    let xml = crate::xml_body(client.get(location.clone()).send().await?).await?;
    device_description(&xml, location)
}

async fn discover_at(
    destination: SocketAddr,
    source: Ipv4Addr,
    window: Duration,
) -> Result<Vec<DiscoveredDevice>, Error> {
    let socket = UdpSocket::bind((source, 0)).await?;
    socket.set_multicast_ttl_v4(2)?;
    let search = format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: {destination}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: ssdp:all\r\n\r\n"
    );
    // Repeat the search because SSDP uses unreliable UDP delivery.
    for _ in 0..2 {
        socket.send_to(search.as_bytes(), destination).await?;
    }
    let deadline = Instant::now() + window;
    let mut packet = [0; 8192];
    let mut locations = BTreeSet::new();
    for _ in 0..512 {
        let (len, _) = match timeout_at(deadline, socket.recv_from(&mut packet)).await {
            Ok(result) => result?,
            Err(_) => break,
        };
        if let Some(url) = response_location(&packet[..len]) {
            locations.insert(url);
            if locations.len() >= 64 {
                tracing::warn!("discovery reached 64 description URLs");
                break;
            }
        }
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()?;
    let mut locations = locations.into_iter();
    let mut pending = JoinSet::new();
    for location in locations.by_ref().take(8) {
        pending.spawn(fetch_device(client.clone(), location));
    }
    let mut devices = BTreeMap::new();
    while let Some(result) = pending.join_next().await {
        match result {
            Ok(Ok(device)) => {
                devices.entry(device.udn.clone()).or_insert(device);
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, "ignoring unsupported or unreachable UPnP description")
            }
            Err(error) => return Err(Error::DiscoveryTask(error)),
        }
        if let Some(location) = locations.next() {
            pending.spawn(fetch_device(client.clone(), location));
        }
    }
    Ok(devices.into_values().collect())
}

/// Chooses the local address routed to the selected TV without sending a UDP
/// datagram. The callback port comes from the bound listener, including port 0.
pub async fn automatic_callback(location: &Url, listener: SocketAddr) -> Result<Url, Error> {
    let ip = if listener.ip().is_unspecified() {
        let host = location
            .host_str()
            .ok_or(Error::Protocol("description has no host"))?;
        let peer = lookup_host((host, location.port_or_known_default().unwrap_or(80)))
            .await?
            .find(|addr| addr.is_ipv4() == listener.is_ipv4())
            .ok_or(Error::Protocol(
                "TV and callback listener have incompatible address families",
            ))?;
        let socket = UdpSocket::bind(SocketAddr::new(listener.ip(), 0)).await?;
        socket.connect(peer).await?;
        socket.local_addr()?.ip()
    } else {
        listener.ip()
    };
    crate::http_url(&format!(
        "http://{}/notify",
        SocketAddr::new(ip, listener.port())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device(udn: &str, name: &str) -> DiscoveredDevice {
        DiscoveredDevice {
            udn: udn.into(),
            name: name.into(),
            description_url: Url::parse("http://127.0.0.1/tv.xml").unwrap(),
        }
    }
    #[test]
    fn parses_case_insensitive_location_and_ignores_non_responses() {
        let url = response_location(b"HTTP/1.1 200 OK\r\nlOcAtIoN: http://192.168.1.2/tv.xml\r\nST: upnp:rootdevice\r\n\r\n").unwrap();
        assert_eq!(url.as_str(), "http://192.168.1.2/tv.xml");
        assert!(response_location(b"NOTIFY * HTTP/1.1\r\nLOCATION: http://tv/\r\n\r\n").is_none());
        assert!(
            response_location(b"HTTP/1.1 200 OK\r\nLOCATION: file:///etc/passwd\r\n\r\n").is_none()
        );
    }
    #[test]
    fn identifies_rendering_control_devices_by_stable_udn_and_name() {
        let xml = "<root xmlns='urn:schemas-upnp-org:device-1-0'><device><friendlyName>Living room TV</friendlyName><UDN>uuid:tv</UDN><serviceList><service><serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType><controlURL>/control</controlURL><eventSubURL>/events</eventSubURL></service></serviceList></device></root>";
        let found = device_description(xml, Url::parse("http://tv/tv.xml").unwrap()).unwrap();
        assert_eq!(found.udn(), "uuid:tv");
        assert_eq!(found.name(), "Living room TV");
        assert!(
            device_description(
                "<root><device><friendlyName>Router</friendlyName></device></root>",
                Url::parse("http://router/").unwrap()
            )
            .is_err()
        );
    }
    #[test]
    fn selection_requires_unique_match_and_prefers_udn_over_name() {
        let devices = vec![
            device("uuid:tv", "Living room TV"),
            device("uuid:speaker", "Speaker"),
        ];
        assert!(matches!(
            select_device(&devices, None),
            Err(SelectionError::Ambiguous(_))
        ));
        assert_eq!(
            select_device(&devices, Some("uuid:tv")).unwrap().udn(),
            "uuid:tv"
        );
        assert_eq!(
            select_device(&devices, Some("Living room TV"))
                .unwrap()
                .udn(),
            "uuid:tv"
        );
        assert!(matches!(
            select_device(&devices, Some("missing")),
            Err(SelectionError::NoMatch(_))
        ));
        assert!(matches!(
            select_device(&[], None),
            Err(SelectionError::NoneFound)
        ));
        let same_name = vec![device("uuid:a", "TV"), device("uuid:b", "TV")];
        assert!(matches!(
            select_device(&same_name, Some("TV")),
            Err(SelectionError::Ambiguous(_))
        ));
        let collision = vec![device("uuid:a", "uuid:b"), device("uuid:b", "TV")];
        assert_eq!(
            select_device(&collision, Some("uuid:b")).unwrap().udn(),
            "uuid:b"
        );
    }
    #[tokio::test]
    async fn udp_discovery_deduplicates_devices_filters_descriptions_and_only_uses_get() {
        use axum::{
            Router,
            http::{Method, Uri},
            routing::any,
        };
        use std::sync::{Arc, Mutex};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let app = Router::new().fallback(any(move |method: Method, uri: Uri| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(method);
                if uri.path() == "/router" { return "<root><device><friendlyName>Router</friendlyName></device></root>"; }
                "<root><device><friendlyName>TV</friendlyName><UDN>uuid:test-tv</UDN><serviceList><service><serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType><controlURL>/control</controlURL><eventSubURL>/events</eventSubURL></service></serviceList></device></root>"
            }
        }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let ssdp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let ssdp_addr = ssdp.local_addr().unwrap();
        let responder = tokio::spawn(async move {
            let mut packet = [0; 2048];
            for _ in 0..2 {
                let (size, source) = ssdp.recv_from(&mut packet).await.unwrap();
                let search = std::str::from_utf8(&packet[..size]).unwrap();
                assert!(search.starts_with("M-SEARCH * HTTP/1.1\r\n"));
                assert!(search.contains("MAN: \"ssdp:discover\"\r\n"));
                assert!(search.contains("ST: ssdp:all\r\n"));
                for path in ["tv", "tv", "duplicate", "router"] {
                    let response =
                        format!("HTTP/1.1 200 OK\r\nLOCATION: http://{http_addr}/{path}\r\n\r\n");
                    ssdp.send_to(response.as_bytes(), source).await.unwrap();
                }
                ssdp.send_to(b"invalid packet", source).await.unwrap();
            }
        });
        let devices = tokio::time::timeout(
            Duration::from_secs(5),
            discover_at(ssdp_addr, Ipv4Addr::LOCALHOST, Duration::from_millis(100)),
        )
        .await
        .unwrap()
        .unwrap();
        responder.await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].udn(), "uuid:test-tv");
        assert_eq!(devices[0].name(), "TV");
        assert_eq!(
            requests.lock().unwrap().as_slice(),
            &[Method::GET, Method::GET, Method::GET]
        );
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn callback_uses_route_to_tv_and_actual_listener_port() {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let callback = automatic_callback(&Url::parse("http://127.0.0.1:9197/dmr").unwrap(), addr)
            .await
            .unwrap();
        assert_eq!(callback.host_str(), Some("127.0.0.1"));
        assert_eq!(callback.port(), Some(addr.port()));
        assert_eq!(callback.path(), "/notify");
        let explicit = "127.0.0.2:9198".parse().unwrap();
        assert_eq!(
            automatic_callback(&Url::parse("http://127.0.0.1/tv").unwrap(), explicit)
                .await
                .unwrap()
                .host_str(),
            Some("127.0.0.2")
        );
    }
}

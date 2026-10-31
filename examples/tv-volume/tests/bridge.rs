use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode},
    response::IntoResponse,
    routing::any,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tanuki_client::{
    ExpiryUpdate, Session,
    protocol::{ClientName, Selector},
};
use tanuki_tv_volume::{Config, run};
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot},
    time::timeout,
};

#[derive(Clone)]
struct Tv {
    callback: Arc<Mutex<Option<String>>>,
    reject_renewal: Arc<AtomicBool>,
    commands: mpsc::UnboundedSender<String>,
    subscriptions: mpsc::UnboundedSender<String>,
}
async fn tv(
    State(tv): State<Tv>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    match method.as_str() {
        "GET" => (StatusCode::OK, HeaderMap::new(), "<root><device><serviceList><service><serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType><controlURL>/control</controlURL><eventSubURL>/events</eventSubURL></service></serviceList></device></root>".to_owned()),
        "SUBSCRIBE" => {
            if let Some(callback) = headers.get("callback") {
                *tv.callback.lock().unwrap() = Some(callback.to_str().unwrap().trim_matches(['<','>']).into());
            }
            tv.subscriptions.send(if headers.contains_key("sid") { "renew" } else { "new" }.into()).unwrap();
            if headers.contains_key("sid") && tv.reject_renewal.load(Ordering::SeqCst) { return (StatusCode::PRECONDITION_FAILED, HeaderMap::new(), String::new()); }
            let mut headers = HeaderMap::new();
            headers.insert("sid", "uuid:test".parse().unwrap());
            headers.insert("timeout", "Second-4".parse().unwrap());
            (StatusCode::OK, headers, String::new())
        },
        "POST" => {
            tv.commands.send(String::from_utf8(body.to_vec()).unwrap()).unwrap();
            (StatusCode::OK, HeaderMap::new(), "<s:Envelope xmlns:s='http://schemas.xmlsoap.org/soap/envelope/'><s:Body><u:SetVolumeResponse xmlns:u='urn:schemas-upnp-org:service:RenderingControl:1'/></s:Body></s:Envelope>".into())
        },
        "UNSUBSCRIBE" => { tv.subscriptions.send("unsubscribe".into()).unwrap(); (StatusCode::OK, HeaderMap::new(), String::new()) },
        _ => (StatusCode::METHOD_NOT_ALLOWED, HeaderMap::new(), String::new()),
    }
}
async fn notify(tv: &Tv, sid: &str, seq: u32, volume: u8) -> StatusCode {
    notify_body(tv, sid, seq, format!("<e:propertyset xmlns:e='urn:schemas-upnp-org:event-1-0'><e:property><LastChange>&lt;Event xmlns='urn:schemas-upnp-org:metadata-1-0/RCS/'&gt;&lt;InstanceID val='0'&gt;&lt;Volume channel='Master' val='{volume}'/&gt;&lt;/InstanceID&gt;&lt;/Event&gt;</LastChange></e:property></e:propertyset>")).await
}
async fn notify_body(tv: &Tv, sid: &str, seq: u32, body: String) -> StatusCode {
    let callback = tv.callback.lock().unwrap().clone().unwrap();
    reqwest::Client::new()
        .request(Method::from_bytes(b"NOTIFY").unwrap(), callback)
        .header("sid", sid)
        .header("seq", seq.to_string())
        .header("nt", "upnp:event")
        .header("nts", "upnp:propchange")
        .body(body)
        .send()
        .await
        .unwrap()
        .status()
}

#[derive(Clone, Copy)]
enum Ending {
    Shutdown,
    SequenceGap,
    RenewalFailure,
    ClaimTaken,
    NoInitialEvent,
    EmptyNotifications,
    ReorderedNotifications,
}

#[tokio::test]
async fn real_server_observes_tv_events_and_controls_without_fabricating_actual_volume() {
    scenario(Ending::Shutdown).await;
}
#[tokio::test]
async fn missed_notification_exits_and_removes_stale_volume() {
    scenario(Ending::SequenceGap).await;
}
#[tokio::test]
async fn failed_renewal_exits_and_removes_stale_volume() {
    scenario(Ending::RenewalFailure).await;
}
#[tokio::test]
async fn displaced_controller_exits_and_releases_session() {
    scenario(Ending::ClaimTaken).await;
}

#[tokio::test]
async fn tv_without_initial_volume_fails_instead_of_polling() {
    scenario(Ending::NoInitialEvent).await;
}

#[tokio::test]
async fn empty_notification_burst_preserves_sequence_and_next_volume() {
    scenario(Ending::EmptyNotifications).await;
}

#[tokio::test]
async fn reordered_notifications_preserve_latest_volume_without_restart() {
    scenario(Ending::ReorderedNotifications).await;
}

async fn scenario(ending: Ending) {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    timeout(Duration::from_secs(15), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tanuki_url = format!("ws://{}/v1/ws", listener.local_addr().unwrap());
        let (server_stop, rx) = oneshot::channel();
        let server = tokio::spawn(tanuki::server::serve_with_core(
            listener,
            Arc::new(Mutex::new(tanuki::core::Core::new())),
            Arc::new(|| tanuki::domain::Timestamp::new(jiff::Timestamp::now())),
            async {
                let _ = rx.await;
            },
        ));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let description_url = format!("http://{}/description.xml", listener.local_addr().unwrap());
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let (subscriptions, mut sub_rx) = mpsc::unbounded_channel();
        let tv_state = Tv {
            callback: Arc::new(Mutex::new(None)),
            reject_renewal: Arc::new(AtomicBool::new(false)),
            commands,
            subscriptions,
        };
        let app = Router::new().fallback(any(tv)).with_state(tv_state.clone());
        let tv_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let callback_url = format!("http://{}/notify", listener.local_addr().unwrap());
        let (stop, rx) = oneshot::channel();
        let bridge = tokio::spawn(run(
            Config {
                description_url,
                callback_url,
                tanuki_url: tanuki_url.clone(),
            },
            listener,
            async {
                let _ = rx.await;
            },
        ));
        let client = Session::connect(
            &tanuki_url,
            ClientName::parse("test dashboard").unwrap(),
            vec![Selector::parse("/tv/**").unwrap()],
        )
        .await
        .unwrap();
        let actual = client.state::<u8>("/tv/volume").unwrap();
        let desired = client.desired::<u8>("/tv/desired-volume").unwrap();
        let mut view = client.observe(&actual).await.unwrap();
        assert!(
            view.recv()
                .await
                .unwrap()
                .unwrap()
                .read(&actual)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            timeout(Duration::from_secs(2), sub_rx.recv())
                .await
                .unwrap()
                .unwrap(),
            "new"
        );
        if matches!(ending, Ending::NoInitialEvent) {
            assert!(matches!(bridge.await.unwrap(), Err(tanuki_tv_volume::Error::Protocol(message)) if message.contains("no initial Master volume")));
            while sub_rx.recv().await.unwrap() != "unsubscribe" {}
            assert!(command_rx.try_recv().is_err());
            client.close().await.unwrap();
            server_stop.send(()).unwrap();
            server.await.unwrap().unwrap();
            tv_task.abort();
            return;
        }
        assert_eq!(notify(&tv_state, "uuid:test", 0, 25).await, StatusCode::OK);
        let snapshot = view.recv().await.unwrap().unwrap();
        assert_eq!(
            snapshot
                .read(&actual)
                .unwrap()
                .unwrap()
                .current()
                .unwrap()
                .value,
            25
        );
        if matches!(ending, Ending::EmptyNotifications | Ending::ReorderedNotifications) {
            if matches!(ending, Ending::ReorderedNotifications) {
                assert_eq!(notify(&tv_state, "uuid:test", 2, 40).await, StatusCode::OK);
                assert_eq!(notify(&tv_state, "uuid:test", 1, 30).await, StatusCode::OK);
            } else {
            // More events than the queue capacity, delivered as an ordinary
            // sequential HTTP burst. Empty events must still advance SEQ.
            let changes = ["<LastChange/>", "<LastChange></LastChange>", "<LastChange> \n </LastChange>", "<LastChange><![CDATA[]]></LastChange>", "<LastChange><![CDATA[ \t ]]></LastChange>"];
            for seq in 1..=130 {
                let change = changes[(seq as usize - 1) % changes.len()];
                let body = format!("<e:propertyset xmlns:e='urn:schemas-upnp-org:event-1-0'><e:property>{change}</e:property></e:propertyset>");
                assert_eq!(notify_body(&tv_state, "uuid:test", seq, body).await, StatusCode::OK, "empty notification {seq} must be accepted");
            }
            assert!(!bridge.is_finished());
            assert_eq!(notify(&tv_state, "uuid:test", 131, 30).await, StatusCode::OK);
            }
            let expected = if matches!(ending, Ending::ReorderedNotifications) { 40 } else { 30 };
            loop {
                if view.recv().await.unwrap().unwrap().read(&actual).unwrap().unwrap().current().unwrap().value == expected { break; }
            }
            assert!(command_rx.try_recv().is_err(), "notification handling must not issue SOAP control requests");
            stop.send(()).unwrap();
            bridge.await.unwrap().unwrap();
            while sub_rx.recv().await.unwrap() != "unsubscribe" {}
            client.close().await.unwrap();
            server_stop.send(()).unwrap();
            server.await.unwrap().unwrap();
            tv_task.abort();
            return;
        }
        let expires = snapshot.read(&actual).unwrap().unwrap().current().unwrap().expires_at.clone().unwrap();
        let remaining = expires.parse::<jiff::Timestamp>().unwrap().since(jiff::Timestamp::now()).unwrap().get_seconds();
        assert!((88..=90).contains(&remaining), "observed volume must expire after bridge failure");
        // A duplicate must not roll back state; malformed input is not acknowledged.
        assert_eq!(notify(&tv_state, "uuid:test", 0, 80).await, StatusCode::OK);
        assert_eq!(notify(&tv_state, "uuid:test", 1, 101).await, StatusCode::BAD_REQUEST);
        desired.submit(&40, ExpiryUpdate::Clear).await.unwrap();
        let command = command_rx.recv().await.unwrap();
        assert!(command.contains("<DesiredVolume>40</DesiredVolume>"));
        assert!(
            timeout(Duration::from_millis(100), view.recv())
                .await
                .is_err(),
            "SOAP acceptance must not publish observed state"
        );
        desired.submit(&101, ExpiryUpdate::Clear).await.unwrap();
        assert!(
            timeout(Duration::from_millis(100), command_rx.recv())
                .await
                .is_err()
        );
        assert_eq!(notify(&tv_state, "uuid:other", 1, 80).await, StatusCode::OK);
        assert!(
            timeout(Duration::from_millis(100), view.recv())
                .await
                .is_err()
        );
        assert_eq!(notify(&tv_state, "uuid:test", 1, 39).await, StatusCode::OK);
        assert_eq!(
            view.recv()
                .await
                .unwrap()
                .unwrap()
                .read(&actual)
                .unwrap()
                .unwrap()
                .current()
                .unwrap()
                .value,
            39
        );
        match ending {
            Ending::NoInitialEvent | Ending::EmptyNotifications | Ending::ReorderedNotifications => unreachable!("handled above"),
            Ending::Shutdown => {
                assert_eq!(sub_rx.recv().await.unwrap(), "renew");
                stop.send(()).unwrap();
                bridge.await.unwrap().unwrap();
            }
            Ending::SequenceGap => {
                assert_eq!(notify(&tv_state, "uuid:test", 3, 41).await, StatusCode::OK);
                assert!(matches!(bridge.await.unwrap(), Err(tanuki_tv_volume::Error::Protocol(message)) if message.contains("sequence gap")));
            }
            Ending::RenewalFailure => {
                tv_state.reject_renewal.store(true, Ordering::SeqCst);
                assert_eq!(sub_rx.recv().await.unwrap(), "renew");
                assert!(matches!(bridge.await.unwrap(), Err(tanuki_tv_volume::Error::Http(_))));
            }
            Ending::ClaimTaken => {
                desired.claim(tanuki_client::ClaimRelease::Immediate).await.unwrap();
                assert!(matches!(bridge.await.unwrap(), Err(tanuki_tv_volume::Error::Protocol(message)) if message.contains("another controller")));
            }
        }
        // Observer may first drain the renewal refresh, then must see removal.
        loop {
            if view.recv().await.unwrap().unwrap().read(&actual).unwrap().is_none() { break; }
        }
        assert_eq!(sub_rx.recv().await.unwrap(), "unsubscribe");
        client.close().await.unwrap();
        server_stop.send(()).unwrap();
        server.await.unwrap().unwrap();
        tv_task.abort();
    })
    .await
    .unwrap();
}

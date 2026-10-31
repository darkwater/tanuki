#![cfg(target_arch = "wasm32")]

use tanuki_client::protocol::*;
use tanuki_client::{
    ClientError, Codec, Connection, ConnectionError, DecodedNode, ExpiryUpdate, Session,
    SessionEnd, SessionOptions,
};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);
const URL: &str = match option_env!("TANUKI_TEST_WS_URL") {
    Some(url) => url,
    None => "ws://127.0.0.1:5168/v1/ws",
};

async fn session(name: &str, prefix: &str, codec: Codec) -> Session {
    Session::connect_with_options(
        URL,
        ClientName::parse(name).unwrap(),
        vec![Selector::parse(prefix).unwrap()],
        SessionOptions {
            codec,
            ..Default::default()
        },
    )
    .await
    .unwrap()
}
fn hello(name: &str) -> ClientMessage {
    ClientMessage::Hello {
        request_id: RequestId::new("hello".into()),
        client: ClientName::parse(name).unwrap(),
        selectors: vec![],
    }
}

// US-09.A1/A2/A4: the real server and real browser sockets, no simulated SDK.
#[wasm_bindgen_test]
async fn sessions_preserve_atomic_batches_typed_bytes_and_removal_in_both_codecs() {
    for (name, codec) in [
        ("browser-json", Codec::Json),
        ("browser-msgpack", Codec::MessagePack),
    ] {
        let prefix = format!("/{name}/**");
        let producer = session(&format!("{name}-producer"), &prefix, codec).await;
        let dashboard = session(&format!("{name}-dashboard"), &prefix, codec).await;
        let a = producer.state::<u16>(&format!("/{name}/hue")).unwrap();
        let b = producer
            .state::<u8>(&format!("/{name}/brightness"))
            .unwrap();
        let bytes = producer
            .state::<serde_bytes::ByteBuf>(&format!("/{name}/bytes"))
            .unwrap();
        let read_a = dashboard.state::<u16>(&a.path().to_string()).unwrap();
        let read_b = dashboard.state::<u8>(&b.path().to_string()).unwrap();
        let mut raw = dashboard.listen_updates().await.unwrap();
        assert!(raw.initial_snapshot().nodes.is_empty());
        let mut observer = dashboard
            .observe_batch([a.path().clone(), b.path().clone()])
            .await
            .unwrap();
        let mut batch = producer.batch();
        batch.publish(&a, &120, ExpiryUpdate::Clear).unwrap();
        batch.publish(&b, &70, ExpiryUpdate::Clear).unwrap();
        batch
            .publish(
                &bytes,
                &serde_bytes::ByteBuf::from(vec![0, 128, 255]),
                ExpiryUpdate::Clear,
            )
            .unwrap();
        let receipt = batch.commit().await.unwrap();
        let update = raw.recv().await.unwrap().unwrap();
        assert_eq!(update.sequence, receipt.sequence);
        assert_eq!(update.changes.len(), 3);
        let ChangeView::Upsert { node: NodeView::State { current }, .. } = update.changes.iter().find(|change| matches!(change, ChangeView::Upsert { topic, .. } if topic.ends_with("/bytes"))).unwrap() else { panic!("missing bytes"); };
        assert_eq!(current.value.as_inner(), &Value::Bytes(vec![0, 128, 255]));
        assert!(observer.recv().await.unwrap().unwrap().nodes().is_empty());
        let snapshot = observer.recv().await.unwrap().unwrap();
        assert_eq!(snapshot.sequence(), receipt.sequence);
        let Some(DecodedNode::State(hue)) = snapshot.read(&read_a).unwrap() else {
            panic!("missing hue");
        };
        let Some(DecodedNode::State(brightness)) = snapshot.read(&read_b).unwrap() else {
            panic!("missing brightness");
        };
        assert_eq!((hue.value, brightness.value), (120, 70));
        a.remove().await.unwrap();
        assert!(
            observer
                .recv()
                .await
                .unwrap()
                .unwrap()
                .read(&read_a)
                .unwrap()
                .is_none()
        );
        assert!(snapshot.read(&read_a).unwrap().is_some());
        // Dropping one projection must not cancel another or its session.
        drop(observer);
        let removal = raw.recv().await.unwrap().unwrap();
        assert!(matches!(&removal.changes[0], ChangeView::Removed { .. }));
        let receipt = b.publish(&80, ExpiryUpdate::Clear).await.unwrap();
        assert_eq!(
            raw.recv().await.unwrap().unwrap().sequence,
            receipt.sequence
        );
        dashboard.close().await.unwrap();
        producer.close().await.unwrap();
        assert!(matches!(
            b.publish(&90, ExpiryUpdate::Clear).await,
            Err(ClientError::SessionGone)
        ));
    }
}

#[wasm_bindgen_test]
async fn direct_connection_waits_for_open_mixes_codecs_and_reports_remote_errors() {
    let mut connection = Connection::open(URL).await.unwrap();
    connection.send(&hello("browser-direct")).await.unwrap();
    assert!(matches!(
        connection.recv().await.unwrap(),
        Some(ServerMessage::Snapshot { .. })
    ));
    for codec in [Codec::Json, Codec::MessagePack] {
        connection
            .send_with_codec(
                &ClientMessage::Write {
                    request_id: RequestId::new("empty".into()),
                    operations: vec![],
                },
                codec,
            )
            .await
            .unwrap();
        assert!(
            matches!(connection.recv().await.unwrap(), Some(ServerMessage::Error { error, .. }) if error.code == "empty_batch")
        );
    }
    connection.close().await.unwrap();
    assert!(connection.recv().await.unwrap().is_none());
    assert!(connection.send(&hello("after-close")).await.is_err());
}

#[wasm_bindgen_test]
async fn replacement_preserves_abnormal_close_code_and_reason() {
    let original = session("browser-replaced", "/browser-replaced/**", Codec::Json).await;
    let mut raw = original.listen_updates().await.unwrap();
    let replacement = session("browser-replaced", "/browser-replaced/**", Codec::Json).await;
    let reason = original.closed().await;
    assert!(
        matches!(&reason, SessionEnd::Connection(error) if matches!(error.as_ref(), ConnectionError::Closed { code: 4001, reason } if reason == "session_replaced"))
    );
    assert!(raw.recv().await.is_err());
    assert!(raw.recv().await.unwrap().is_none());
    assert!(original.close().await.is_err());
    replacement.close().await.unwrap();
}

#[wasm_bindgen_test]
async fn invalid_urls_and_oversized_writes_fail_locally() {
    assert!(Connection::open("ws://[").await.is_err());
    let producer = session("browser-limits", "/browser-limits/**", Codec::Json).await;
    let topic = producer.state::<String>("/browser-limits/large").unwrap();
    assert!(matches!(
        topic
            .publish(&"x".repeat(1024 * 1024), ExpiryUpdate::Clear)
            .await,
        Err(ClientError::Connection(ConnectionError::MessageTooLarge))
    ));
    assert!(producer.termination().is_none());
    topic
        .publish(&"small".into(), ExpiryUpdate::Clear)
        .await
        .unwrap();
    producer.close().await.unwrap();
}

#[wasm_bindgen_test]
async fn stalled_handshake_obeys_browser_deadline() {
    let url = match option_env!("TANUKI_TEST_STALL_URL") {
        Some(url) => url,
        None => panic!("use tests/run-browser.py to provide a stalled peer"),
    };
    let result = Session::connect_with_options(
        url,
        ClientName::parse("browser-timeout").unwrap(),
        vec![],
        SessionOptions {
            connect_timeout: std::time::Duration::from_millis(20),
            ..Default::default()
        },
    )
    .await;
    assert!(matches!(result, Err(ClientError::ConnectTimeout)));
}

#[wasm_bindgen_test]
async fn raw_lag_is_local_and_reported_once() {
    let client = Session::connect_with_options(
        URL,
        ClientName::parse("browser-lag").unwrap(),
        vec![Selector::parse("/browser-lag/**").unwrap()],
        SessionOptions {
            raw_capacity: std::num::NonZeroUsize::new(1).unwrap(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let topic = client.state::<u8>("/browser-lag/value").unwrap();
    let mut slow = client.listen_updates().await.unwrap();
    let mut healthy = client.listen_updates().await.unwrap();
    for value in [1, 2] {
        let receipt = topic.publish(&value, ExpiryUpdate::Clear).await.unwrap();
        assert_eq!(
            healthy.recv().await.unwrap().unwrap().sequence,
            receipt.sequence
        );
    }
    // Receiving the second batch on healthy establishes that the same driver
    // already attempted delivery to slow; no timeout-based absence assertion.
    assert!(matches!(slow.recv().await, Err(SessionEnd::Lagged)));
    assert!(slow.recv().await.unwrap().is_none());
    let receipt = topic.publish(&3, ExpiryUpdate::Clear).await.unwrap();
    assert_eq!(
        healthy.recv().await.unwrap().unwrap().sequence,
        receipt.sequence
    );
    client.close().await.unwrap();
}

#[wasm_bindgen_test]
async fn zero_write_deadline_rejects_before_sending() {
    let client = Session::connect_with_options(
        URL,
        ClientName::parse("browser-unsent").unwrap(),
        vec![Selector::parse("/browser-unsent/**").unwrap()],
        SessionOptions {
            write_timeout: std::time::Duration::ZERO,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let topic = client.state::<u8>("/browser-unsent/value").unwrap();
    assert!(matches!(
        topic.publish(&1, ExpiryUpdate::Clear).await,
        Err(ClientError::UnsentTimeout)
    ));
    // Registration is serialized after the rejected command, proving no local
    // mutation appeared while the connection stayed live.
    assert!(
        client
            .listen_updates()
            .await
            .unwrap()
            .initial_snapshot()
            .nodes
            .is_empty()
    );
    assert!(client.termination().is_none());
    client.close().await.unwrap();
}

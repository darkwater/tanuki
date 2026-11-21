#![cfg(target_arch = "wasm32")]

use std::time::Duration;
use tanuki_client::protocol::*;
use tanuki_client::{
    ClientError, Closed, Codec, Connection, ConnectionError, Disconnect, Event, Node, Reconnect,
    Resync, Session, StateTopic, Status,
};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);
const URL: &str = match option_env!("TANUKI_TEST_WS_URL") {
    Some(url) => url,
    None => "ws://127.0.0.1:5168/v1/ws",
};

async fn session(name: &str, codec: Codec) -> Session {
    Session::builder(URL, name)
        .codec(codec)
        .reconnect(Reconnect::Never)
        .connect()
        .await
        .unwrap()
}

fn hello(name: &str) -> ClientMessage {
    ClientMessage::Hello {
        request_id: RequestId::new("hello".into()),
        client: ClientName::parse(name).unwrap(),
        selectors: vec![],
        session: SessionMode::Managed,
    }
}

// US-09.A1/A2/A4: the real server and real browser sockets, no simulated SDK.
#[wasm_bindgen_test]
async fn sessions_preserve_atomic_batches_typed_bytes_and_removal_in_both_codecs() {
    for (name, codec) in [
        ("browser-json", Codec::Json),
        ("browser-msgpack", Codec::MessagePack),
    ] {
        let producer = session(&format!("{name}-producer"), codec).await;
        let dashboard = session(&format!("{name}-dashboard"), codec).await;
        let hue = StateTopic::<u16>::new(&format!("/{name}/hue")).unwrap();
        let brightness = StateTopic::<u8>::new(&format!("/{name}/brightness")).unwrap();
        let bytes = StateTopic::<serde_bytes::ByteBuf>::new(&format!("/{name}/bytes")).unwrap();
        let mut listener = dashboard.listen(format!("/{name}/**")).unwrap();
        let Some(Event::Snapshot { snapshot, reason }) = listener.recv().await else {
            panic!("a listener starts with a baseline");
        };
        assert_eq!(reason, Resync::Started);
        assert!(snapshot.is_empty());
        let mut observer = dashboard.observe((&hue, &brightness)).unwrap();

        let receipt = producer
            .batch()
            .publish(&hue, &120)
            .publish(&brightness, &70)
            .publish(&bytes, &serde_bytes::ByteBuf::from(vec![0, 128, 255]))
            .commit()
            .await
            .unwrap();
        let Some(Event::Update(update)) = listener.recv().await else {
            panic!("the batch arrives as one update");
        };
        assert_eq!(update.sequence, receipt.sequence);
        assert_eq!(update.changes.len(), 3);
        let bytes_change = update
            .changes
            .iter()
            .find(|change| change.topic() == bytes.path())
            .unwrap();
        let ChangeView::Upsert { node, .. } = bytes_change else {
            panic!("bytes are an upsert");
        };
        assert_eq!(
            node.current().unwrap().value.as_inner(),
            &Value::Bytes(vec![0, 128, 255])
        );

        let snapshot = loop {
            let snapshot = observer.recv().await.unwrap();
            if snapshot.sequence() >= receipt.sequence {
                break snapshot;
            }
        };
        let Some(Node::State(read_hue)) = snapshot.read(&hue).unwrap() else {
            panic!("missing hue");
        };
        assert_eq!(
            (read_hue.value, snapshot.get(&brightness).unwrap()),
            (120, Some(70))
        );

        producer.remove(&hue).await.unwrap();
        assert!(observer.recv().await.unwrap().get(&hue).unwrap().is_none());
        assert!(
            snapshot.get(&hue).unwrap().is_some(),
            "snapshots never change"
        );
        // Dropping one observer must not affect a listener or the session.
        drop(observer);
        let Some(Event::Update(removal)) = listener.recv().await else {
            panic!("removal arrives");
        };
        assert!(matches!(&removal.changes[0], ChangeView::Removed { .. }));
        let receipt = producer.publish(&brightness, &80).await.unwrap();
        let Some(Event::Update(update)) = listener.recv().await else {
            panic!("update arrives");
        };
        assert_eq!(update.sequence, receipt.sequence);
        dashboard.close().await;
        producer.close().await;
        assert!(matches!(
            producer.publish(&brightness, &90).await,
            Err(ClientError::Closed(Closed::ByClient))
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
            matches!(connection.recv().await.unwrap(), Some(ServerMessage::Error { error, .. }) if error.code == ErrorCode::EmptyBatch)
        );
    }
    connection.close().await.unwrap();
    assert!(connection.recv().await.unwrap().is_none());
    assert!(connection.send(&hello("after-close")).await.is_err());
}

#[wasm_bindgen_test]
async fn a_replaced_session_ends_and_its_listeners_finish() {
    let original = session("browser-replaced", Codec::Json).await;
    let mut listener = original.listen("/browser-replaced/**").unwrap();
    let replacement = session("browser-replaced", Codec::Json).await;
    assert!(matches!(original.closed().await, Closed::Replaced));
    while listener.recv().await.is_some() {}
    replacement.close().await;
}

#[wasm_bindgen_test]
async fn invalid_urls_and_oversized_writes_fail_locally() {
    assert!(Connection::open("ws://[").await.is_err());
    let producer = session("browser-limits", Codec::Json).await;
    let topic = StateTopic::<String>::new("/browser-limits/large").unwrap();
    assert!(matches!(
        producer.publish(&topic, &"x".repeat(1024 * 1024)).await,
        Err(ClientError::Encode(ConnectionError::MessageTooLarge))
    ));
    assert!(matches!(producer.status().current(), Status::Connected));
    producer.publish(&topic, &"small".into()).await.unwrap();
    producer.close().await;
}

#[wasm_bindgen_test]
async fn stalled_handshake_obeys_browser_deadline() {
    let url = match option_env!("TANUKI_TEST_STALL_URL") {
        Some(url) => url,
        None => panic!("use tests/run-browser.py to provide a stalled peer"),
    };
    let result = Session::builder(url, "browser-timeout")
        .connect_timeout(Duration::from_millis(20))
        .reconnect(Reconnect::Never)
        .connect()
        .await;
    assert!(matches!(
        result,
        Err(ClientError::Connect(Disconnect::Timeout))
    ));
}

#[wasm_bindgen_test]
async fn a_lagging_listener_is_resynchronised_without_affecting_another() {
    let client = Session::builder(URL, "browser-lag")
        .listener_capacity(1)
        .reconnect(Reconnect::Never)
        .connect()
        .await
        .unwrap();
    let topic = StateTopic::<u8>::new("/browser-lag/value").unwrap();
    let mut slow = client.listen(&topic).unwrap();
    let mut healthy = client.listen(&topic).unwrap();
    assert!(matches!(healthy.recv().await, Some(Event::Snapshot { .. })));
    for value in [1, 2] {
        let receipt = client.publish(&topic, &value).await.unwrap();
        let Some(Event::Update(update)) = healthy.recv().await else {
            panic!("the healthy listener keeps up");
        };
        assert_eq!(update.sequence, receipt.sequence);
    }
    // The unread baseline filled slow's queue: each update replaced it with
    // the current state instead of ending the listener.
    let Some(Event::Snapshot { snapshot, reason }) = slow.recv().await else {
        panic!("slow is resynchronised");
    };
    assert_eq!(reason, Resync::Lagged);
    assert_eq!(snapshot.get(&topic).unwrap(), Some(2));
    let receipt = client.publish(&topic, &3).await.unwrap();
    let Some(Event::Update(update)) = slow.recv().await else {
        panic!("slow continues after resynchronising");
    };
    assert_eq!(update.sequence, receipt.sequence);
    client.close().await;
}

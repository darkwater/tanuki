#![cfg(target_arch = "wasm32")]
//! The browser connection against a real server (run with tests/run-browser.py).

use tanuki_client_core::protocol::*;
use tanuki_client_core::{Codec, Connection, ConnectionError};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);
const URL: &str = match option_env!("TANUKI_TEST_WS_URL") {
    Some(url) => url,
    None => "ws://127.0.0.1:5168/v1/ws",
};

fn hello(name: &str, selectors: &[&str], session: SessionMode) -> ClientMessage {
    ClientMessage::Hello {
        request_id: RequestId::new("hello".into()),
        client: ClientName::parse(name).unwrap(),
        selectors: selectors
            .iter()
            .map(|s| Selector::parse(s).unwrap())
            .collect(),
        session,
    }
}

fn publish(request_id: &str, topic: &str, value: Value) -> ClientMessage {
    ClientMessage::Write {
        request_id: RequestId::new(request_id.into()),
        operations: vec![WireOperation::PublishState {
            topic: TopicPath::parse(topic).unwrap(),
            value,
            expiry: None,
        }],
    }
}

async fn recv(connection: &mut Connection) -> ServerMessage {
    connection.recv().await.unwrap().expect("a message")
}

// Hello, own update before the reply, both codecs, bytes, and remote errors.
#[wasm_bindgen_test]
async fn exchanges_hello_updates_replies_and_errors_in_both_codecs() {
    for (name, codec) in [
        ("browser-json", Codec::Json),
        ("browser-msgpack", Codec::MessagePack),
    ] {
        let mut connection = Connection::open_with_codec(URL, codec).await.unwrap();
        let selector = format!("/{name}/**");
        connection
            .send(&hello(name, &[&selector], SessionMode::Stateless))
            .await
            .unwrap();
        assert!(matches!(
            recv(&mut connection).await,
            ServerMessage::Snapshot { .. }
        ));
        let bytes = Value::Bytes(vec![0, 128, 255]);
        connection
            .send(&publish("w1", &format!("/{name}/bytes"), bytes.clone()))
            .await
            .unwrap();
        let ServerMessage::Update { changes, .. } = recv(&mut connection).await else {
            panic!("the write's own update comes before its reply");
        };
        let ChangeView::Upsert { node, .. } = &changes[0] else {
            panic!("an upsert");
        };
        assert_eq!(&node.current().unwrap().value, &bytes);
        assert!(matches!(
            recv(&mut connection).await,
            ServerMessage::Reply { request_id, .. } if request_id.as_str() == "w1"
        ));

        connection
            .send(&ClientMessage::Write {
                request_id: RequestId::new("empty".into()),
                operations: vec![],
            })
            .await
            .unwrap();
        assert!(matches!(
            recv(&mut connection).await,
            ServerMessage::Error { error, .. } if error.code == ErrorCode::EmptyBatch
        ));
        connection.close().await.unwrap();
        assert!(connection.recv().await.unwrap().is_none());
        assert!(
            connection
                .send(&hello(name, &[], SessionMode::Stateless))
                .await
                .is_err()
        );
    }
}

// The halves work independently: a receiving loop and a separate sender.
#[wasm_bindgen_test]
async fn split_halves_send_and_receive_independently() {
    let connection = Connection::open(URL).await.unwrap();
    let (sender, mut receiver) = connection.split();
    sender
        .send(&hello(
            "browser-split",
            &["/browser-split/**"],
            SessionMode::Stateless,
        ))
        .await
        .unwrap();
    assert!(matches!(
        receiver.recv().await.unwrap(),
        Some(ServerMessage::Snapshot { .. })
    ));
    let other = sender.clone();
    other
        .send(&publish("w1", "/browser-split/value", Value::Integer(7)))
        .await
        .unwrap();
    assert!(matches!(
        receiver.recv().await.unwrap(),
        Some(ServerMessage::Update { .. })
    ));
    sender.close().await.unwrap();
    while receiver.recv().await.unwrap().is_some() {}
}

#[wasm_bindgen_test]
async fn a_replaced_managed_session_closes_with_4001() {
    let mut original = Connection::open(URL).await.unwrap();
    original
        .send(&hello("browser-replaced", &[], SessionMode::Managed))
        .await
        .unwrap();
    recv(&mut original).await;
    let mut replacement = Connection::open(URL).await.unwrap();
    replacement
        .send(&hello("browser-replaced", &[], SessionMode::Managed))
        .await
        .unwrap();
    recv(&mut replacement).await;
    assert!(matches!(
        original.recv().await,
        Err(ConnectionError::Closed { code: 4001, reason }) if reason == "session_replaced"
    ));
    replacement.close().await.unwrap();
}

#[wasm_bindgen_test]
async fn invalid_urls_and_oversized_messages_fail_locally() {
    assert!(Connection::open("ws://[").await.is_err());
    let mut connection = Connection::open(URL).await.unwrap();
    connection
        .send(&hello("browser-limits", &[], SessionMode::Stateless))
        .await
        .unwrap();
    recv(&mut connection).await;
    let large = Value::String("x".repeat(1024 * 1024));
    assert!(matches!(
        connection
            .send(&publish("big", "/browser-limits/large", large))
            .await,
        Err(ConnectionError::MessageTooLarge)
    ));
    // Nothing was sent; the connection is still usable.
    connection
        .send(&publish(
            "small",
            "/browser-limits/large",
            Value::Integer(1),
        ))
        .await
        .unwrap();
    assert!(matches!(
        recv(&mut connection).await,
        ServerMessage::Reply { .. }
    ));
    connection.close().await.unwrap();
}

// The reconnecting client's background task and timers on the browser event
// loop: connect, a correlated write, a rejected write, and a normal close.
#[wasm_bindgen_test]
async fn the_client_connects_writes_and_closes_without_a_tokio_runtime() {
    use tanuki_client_core::{Client, ClientError, Closed, Event, SnapshotCause};
    let (client, mut events) = Client::builder(URL, ClientName::parse("browser-client").unwrap())
        .selectors([Selector::parse("/browser-client/*").unwrap()])
        .start();
    assert!(matches!(
        events.next().await,
        Some(Event::Snapshot {
            cause: SnapshotCause::Connected,
            ..
        })
    ));
    let commit = client
        .write(vec![WireOperation::PublishState {
            topic: TopicPath::parse("/browser-client/value").unwrap(),
            value: Value::Integer(5),
            expiry: None,
        }])
        .await
        .unwrap();
    assert!(matches!(
        events.next().await,
        Some(Event::Update(update)) if update.sequence == commit.sequence
    ));
    assert!(matches!(
        client.write(vec![]).await,
        Err(ClientError::Rejected(error)) if error.code == ErrorCode::EmptyBatch
    ));
    client.close().await;
    assert!(matches!(
        events.next().await,
        Some(Event::Closed(Closed::Requested))
    ));
    assert!(events.next().await.is_none());
}

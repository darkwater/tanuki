//! The Rust protocol client (`tanuki_client_core::Connection`) against the real
//! server over loopback WebSockets.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tanuki::{
    core::Core,
    domain::Timestamp,
    server::serve_with_core_and_timeouts,
    transport::{Heartbeat, SharedCore, WebSocketTimeouts},
};
use tanuki_client_core::protocol::*;
use tanuki_client_core::{Codec, Connection};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle, time::timeout};

struct Server {
    url: String,
    core: SharedCore,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}

impl Server {
    async fn start() -> Self {
        Self::start_with(WebSocketTimeouts::default()).await
    }

    async fn start_with(timeouts: WebSocketTimeouts) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let core: SharedCore = Arc::new(Mutex::new(Core::new()));
        let (shutdown, stopped) = oneshot::channel();
        let task = tokio::spawn(serve_with_core_and_timeouts(
            listener,
            core.clone(),
            Arc::new(|| Timestamp::new(jiff::Timestamp::now())),
            timeouts,
            async {
                let _ = stopped.await;
            },
        ));
        Self {
            url: format!("ws://{address}/v1/ws"),
            core,
            shutdown,
            task,
        }
    }

    async fn stop(self) {
        let _ = self.shutdown.send(());
        self.task.await.unwrap().unwrap();
    }
}

async fn soon<T>(future: impl std::future::Future<Output = T>) -> T {
    timeout(Duration::from_secs(5), future)
        .await
        .expect("timed out")
}

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

async fn next(connection: &mut Connection) -> ServerMessage {
    soon(connection.recv())
        .await
        .unwrap()
        .expect("connection ended")
}

#[tokio::test]
async fn a_connection_exchanges_hello_update_reply_and_errors_in_both_codecs() {
    let server = Server::start().await;
    for codec in [Codec::Json, Codec::MessagePack] {
        let mut connection = Connection::open_with_codec(&server.url, codec)
            .await
            .unwrap();
        connection
            .send(&hello(
                &format!("direct {codec:?}"),
                &["/battery/*"],
                SessionMode::Managed,
            ))
            .await
            .unwrap();
        assert!(matches!(
            next(&mut connection).await,
            ServerMessage::Snapshot { request_id, .. } if request_id.as_str() == "hello"
        ));
        // Mixed codecs: each request frame chooses its own reply codec.
        let other = if codec == Codec::Json {
            Codec::MessagePack
        } else {
            Codec::Json
        };
        connection
            .send_with_codec(
                &publish("write", "/battery/laptop", Value::Integer(82)),
                other,
            )
            .await
            .unwrap();
        // The write's own update arrives before its reply.
        assert!(matches!(
            next(&mut connection).await,
            ServerMessage::Update { changes, .. } if changes.len() == 1
        ));
        assert!(matches!(
            next(&mut connection).await,
            ServerMessage::Reply { request_id, .. } if request_id.as_str() == "write"
        ));
        connection
            .send(&ClientMessage::Write {
                request_id: RequestId::new("invalid".into()),
                operations: vec![],
            })
            .await
            .unwrap();
        assert!(matches!(
            next(&mut connection).await,
            ServerMessage::Error { request_id: Some(id), error }
                if id.as_str() == "invalid" && error.code == ErrorCode::EmptyBatch
        ));
        connection.close().await.unwrap();
        assert!(soon(connection.recv()).await.unwrap().is_none());
    }
    server.stop().await;
}

#[tokio::test]
async fn both_codecs_preserve_bytes_floats_and_literal_tag_maps() {
    let values = [
        Value::Float(FiniteF64::new(2.291712365432881e-9).unwrap()),
        Value::Bytes(vec![0, 255]),
        Value::Map(BTreeMap::from([(
            "$bytes".into(),
            Value::String("literal".into()),
        )])),
        Value::Integer(i64::MIN),
        Value::Null,
    ];
    let server = Server::start().await;
    for codec in [Codec::Json, Codec::MessagePack] {
        let mut writer = Connection::open_with_codec(&server.url, codec)
            .await
            .unwrap();
        writer
            .send(&hello(
                "codec writer",
                &["/codec/probe"],
                SessionMode::Stateless,
            ))
            .await
            .unwrap();
        next(&mut writer).await;
        for value in &values {
            writer
                .send(&publish("w", "/codec/probe", value.clone()))
                .await
                .unwrap();
            // Through an update...
            let ServerMessage::Update { changes, .. } = next(&mut writer).await else {
                panic!("expected the write's update");
            };
            let ChangeView::Upsert { node, .. } = &changes[0] else {
                panic!("expected an upsert");
            };
            assert_eq!(&node.current().unwrap().value, value);
            next(&mut writer).await; // reply

            // ...and through a fresh snapshot.
            let mut reader = Connection::open_with_codec(&server.url, codec)
                .await
                .unwrap();
            reader
                .send(&hello(
                    "codec reader",
                    &["/codec/probe"],
                    SessionMode::Stateless,
                ))
                .await
                .unwrap();
            let ServerMessage::Snapshot { nodes, .. } = next(&mut reader).await else {
                panic!("expected the hello snapshot");
            };
            let node = &nodes[&TopicPath::parse("/codec/probe").unwrap()];
            assert_eq!(&node.current().unwrap().value, value);
            reader.close().await.unwrap();
        }
        writer.close().await.unwrap();
    }
    server.stop().await;
}

// A receiver in its own task keeps answering server pings while another task
// sends, and an idle split connection survives the silence window.
#[tokio::test]
async fn split_halves_work_from_separate_tasks_and_keep_the_connection_alive() {
    let server = Server::start_with(WebSocketTimeouts {
        hello: Some(Duration::from_secs(1)),
        heartbeat: Some(Heartbeat {
            interval: Duration::from_millis(50),
            silence: Duration::from_millis(300),
        }),
        send: Some(Duration::from_secs(1)),
    })
    .await;
    let connection = Connection::open(&server.url).await.unwrap();
    let (sender, mut receiver) = connection.split();
    sender
        .send(&hello("split", &["/split/*"], SessionMode::Managed))
        .await
        .unwrap();
    let (messages, mut received) = tokio::sync::mpsc::unbounded_channel();
    let reader = tokio::spawn(async move {
        while let Some(message) = receiver.recv().await.unwrap() {
            messages.send(message).unwrap();
        }
    });
    assert!(matches!(
        soon(received.recv()).await,
        Some(ServerMessage::Snapshot { .. })
    ));

    // Idle for three silence windows: only the receiver's pongs keep it open.
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(server.core.lock().unwrap().managed_session_count(), 1);

    let writer = sender.clone();
    tokio::spawn(async move {
        writer
            .send(&publish("w", "/split/value", Value::Integer(1)))
            .await
            .unwrap();
    })
    .await
    .unwrap();
    assert!(matches!(
        soon(received.recv()).await,
        Some(ServerMessage::Update { .. })
    ));
    assert!(matches!(
        soon(received.recv()).await,
        Some(ServerMessage::Reply { .. })
    ));
    sender.close().await.unwrap();
    soon(reader).await.unwrap();
    server.stop().await;
}

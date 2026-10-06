use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use jiff::Timestamp as JiffTimestamp;
use serde_json::{Value as RawJson, json};
use tanuki::{
    core::{Core, SchemaInstallMode},
    domain::{Node, Selection, Selector, Timestamp, TopicPath, Value},
    link::{LinkDefinition, LinkName},
    protocol::{DiagnosticView, JsonValue, NodeView, ServerMessage},
    schema::{Enforcement, NullPolicy, Schema, SchemaName, SchemaRule, ValueValidator},
    server::serve_with_core,
    transport::{Clock, SharedCore},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
    time::{Duration, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, protocol::frame::coding::CloseCode},
};

type TestSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct TestServer {
    address: std::net::SocketAddr,
    core: SharedCore,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<std::io::Result<()>>,
}

impl TestServer {
    async fn start() -> Self {
        let clock: Clock =
            Arc::new(|| Timestamp::new(JiffTimestamp::from_second(1_700_000_000).unwrap()));
        Self::start_with_clock(clock).await
    }

    async fn start_with_clock(clock: Clock) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let core = Arc::new(Mutex::new(Core::new()));
        let (shutdown, stopped) = oneshot::channel();
        let task = tokio::spawn(serve_with_core(listener, core.clone(), clock, async move {
            let _ = stopped.await;
        }));
        Self {
            address,
            core,
            shutdown: Some(shutdown),
            task,
        }
    }

    async fn connect(&self) -> TestSocket {
        connect_async(format!("ws://{}/v1/ws", self.address))
            .await
            .unwrap()
            .0
    }

    async fn post_json(&self, path: &str, client: &str, body: RawJson) -> RawJson {
        let body = serde_json::to_vec(&body).unwrap();
        let mut stream = TcpStream::connect(self.address).await.unwrap();
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nTanuki-Client: {client}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.address,
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let separator = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
        serde_json::from_slice(&response[separator + 4..]).unwrap()
    }

    async fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.await.unwrap().unwrap();
    }
}

async fn send_json(socket: &mut TestSocket, value: RawJson) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}

async fn recv_json(socket: &mut TestSocket) -> ServerMessage {
    let message = timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("server response timed out")
        .expect("WebSocket closed")
        .expect("WebSocket read failed");
    match message {
        Message::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected text message, received {other:?}"),
    }
}

async fn send_hello(socket: &mut TestSocket, client: &str, selectors: &[&str]) {
    send_json(
        socket,
        json!({
            "type": "hello",
            "request_id": "hello-1",
            "client": client,
            "selectors": selectors,
        }),
    )
    .await;
}

fn battery_write(request_id: &str, value: i64) -> RawJson {
    json!({
        "type": "write",
        "request_id": request_id,
        "operations": [{
            "op": "publish_state",
            "topic": "/battery/laptop",
            "value": value,
            "expiry": {"mode": "clear"}
        }]
    })
}

#[tokio::test]
async fn websocket_writes_cannot_bypass_an_installed_schema() {
    let server = TestServer::start().await;
    server
        .core
        .lock()
        .unwrap()
        .install_schema(
            Schema::new(
                SchemaName::parse("battery").unwrap(),
                vec![
                    SchemaRule::new(
                        Selector::parse("/battery/*").unwrap(),
                        Enforcement::Deny,
                        ValueValidator::integer_range(0, 100, NullPolicy::Deny).unwrap(),
                    )
                    .unwrap(),
                ],
            )
            .unwrap(),
            SchemaInstallMode::RejectInvalid,
            Timestamp::new(JiffTimestamp::from_second(1_700_000_000).unwrap()),
        )
        .unwrap();
    let mut socket = server.connect().await;
    send_hello(&mut socket, "laptop", &["/battery/*"]).await;
    assert!(matches!(
        recv_json(&mut socket).await,
        ServerMessage::Snapshot { .. }
    ));

    send_json(&mut socket, battery_write("invalid", 101)).await;
    assert!(matches!(
        recv_json(&mut socket).await,
        ServerMessage::Error { request_id: Some(request_id), error }
            if request_id.as_str() == "invalid" && error.code == "schema_violation"
    ));

    socket.close(None).await.unwrap();
    wait_for_session_count(&server.core, 0).await;
    server.stop().await;
}

#[tokio::test]
async fn websocket_alias_write_updates_canonical_and_link_subscribers_atomically() {
    let server = TestServer::start().await;
    server
        .core
        .lock()
        .unwrap()
        .install_link(
            LinkDefinition::new(
                LinkName::parse("living-room").unwrap(),
                TopicPath::parse("/rooms/living-room").unwrap(),
                TopicPath::parse("/devices/lamp").unwrap(),
            )
            .unwrap(),
            Timestamp::new(JiffTimestamp::from_second(1_700_000_000).unwrap()),
        )
        .unwrap();
    let mut socket = server.connect().await;
    send_hello(
        &mut socket,
        "wall panel",
        &["/devices/lamp/**", "/rooms/living-room/**"],
    )
    .await;
    assert!(matches!(
        recv_json(&mut socket).await,
        ServerMessage::Snapshot { .. }
    ));

    send_json(
        &mut socket,
        json!({
            "type": "write",
            "request_id": "alias-write",
            "operations": [{
                "op": "publish_state",
                "topic": "/rooms/living-room/power",
                "value": true,
                "expiry": {"mode": "clear"}
            }]
        }),
    )
    .await;
    assert!(matches!(
        recv_json(&mut socket).await,
        ServerMessage::Reply { request_id, .. } if request_id.as_str() == "alias-write"
    ));
    assert_update_topics(
        &mut socket,
        &["/devices/lamp/power", "/rooms/living-room/power"],
    )
    .await;

    socket.close(None).await.unwrap();
    wait_for_session_count(&server.core, 0).await;
    server.stop().await;
}

#[tokio::test]
async fn json_hello_snapshot_then_write_reply_then_atomic_update() {
    let server = TestServer::start().await;
    let mut socket = server.connect().await;
    send_hello(&mut socket, "laptop", &["/battery/*"]).await;

    assert!(matches!(
        recv_json(&mut socket).await,
        ServerMessage::Snapshot { request_id, ref nodes, .. }
            if request_id.as_str() == "hello-1" && nodes.is_empty()
    ));

    send_json(&mut socket, battery_write("write-1", 87)).await;
    assert!(matches!(
        recv_json(&mut socket).await,
        ServerMessage::Reply { request_id, .. } if request_id.as_str() == "write-1"
    ));
    let ServerMessage::Update { changes, .. } = recv_json(&mut socket).await else {
        panic!("the committed update must follow its correlated reply");
    };
    assert_eq!(changes.len(), 1);

    socket.close(None).await.unwrap();
    wait_for_session_count(&server.core, 0).await;
    server.stop().await;
}

#[tokio::test]
async fn same_name_http_is_stateless_but_duplicate_websocket_replaces_session() {
    let server = TestServer::start().await;
    let mut original = server.connect().await;
    send_hello(&mut original, "laptop", &["/battery/*"]).await;
    assert!(matches!(
        recv_json(&mut original).await,
        ServerMessage::Snapshot { .. }
    ));

    let response = server
        .post_json(
            "/v1/state/battery/phone",
            "laptop",
            json!({"value": 42, "expiry": {"mode": "clear"}}),
        )
        .await;
    assert_eq!(response["ok"], true);
    assert_eq!(server.core.lock().unwrap().managed_session_count(), 1);

    send_json(&mut original, battery_write("still-current", 88)).await;
    assert!(matches!(
        recv_json(&mut original).await,
        ServerMessage::Update { .. }
    ));
    assert!(matches!(
        recv_json(&mut original).await,
        ServerMessage::Reply { request_id, .. } if request_id.as_str() == "still-current"
    ));
    assert!(matches!(
        recv_json(&mut original).await,
        ServerMessage::Update { .. }
    ));

    let mut replacement = server.connect().await;
    send_hello(&mut replacement, "laptop", &["/battery/*"]).await;
    let ServerMessage::Snapshot { warnings, .. } = recv_json(&mut replacement).await else {
        panic!("replacement hello must receive a snapshot acknowledgement");
    };
    assert!(matches!(
        warnings.as_slice(),
        [DiagnosticView::SessionReplaced { client, .. }] if client == "laptop"
    ));

    let closed = timeout(Duration::from_secs(2), original.next())
        .await
        .expect("replaced socket was not closed")
        .expect("replaced socket stream ended before close frame")
        .expect("replaced socket read failed");
    assert!(matches!(
        closed,
        Message::Close(Some(frame)) if frame.code == CloseCode::Library(4001)
    ));

    send_json(&mut replacement, battery_write("replacement-write", 89)).await;
    assert!(matches!(
        recv_json(&mut replacement).await,
        ServerMessage::Reply { request_id, .. } if request_id.as_str() == "replacement-write"
    ));
    replacement.close(None).await.unwrap();
    wait_for_session_count(&server.core, 0).await;
    server.stop().await;
}

#[tokio::test]
async fn messagepack_uses_the_same_snapshot_reply_and_update_messages() {
    let server = TestServer::start().await;
    let mut socket = server.connect().await;
    let hello = json!({
        "type": "hello",
        "request_id": "binary-hello",
        "client": "binary laptop",
        "selectors": ["/battery/*"]
    });
    socket
        .send(Message::Binary(
            rmp_serde::to_vec_named(&hello).unwrap().into(),
        ))
        .await
        .unwrap();
    assert!(matches!(
        recv_messagepack(&mut socket).await,
        ServerMessage::Snapshot { request_id, .. } if request_id.as_str() == "binary-hello"
    ));

    let write = battery_write("binary-write", 91);
    socket
        .send(Message::Binary(
            rmp_serde::to_vec_named(&write).unwrap().into(),
        ))
        .await
        .unwrap();
    assert!(matches!(
        recv_messagepack(&mut socket).await,
        ServerMessage::Reply { request_id, .. } if request_id.as_str() == "binary-write"
    ));
    let ServerMessage::Update { changes, .. } = recv_messagepack(&mut socket).await else {
        panic!("binary subscriber must receive the committed update");
    };
    let NodeView::State { current } = (match changes.as_slice() {
        [tanuki::protocol::ChangeView::Upsert { node, .. }] => node,
        other => panic!("unexpected changes: {other:?}"),
    }) else {
        panic!("battery update must be state");
    };
    assert_eq!(current.value, JsonValue::new(Value::Integer(91)));

    socket.close(None).await.unwrap();
    wait_for_session_count(&server.core, 0).await;
    server.stop().await;
}

#[tokio::test]
async fn each_request_frame_selects_its_reply_codec() {
    let server = TestServer::start().await;
    let mut socket = server.connect().await;
    send_hello(&mut socket, "mixed codec", &["/battery/*"]).await;
    let _ = recv_json(&mut socket).await;

    socket
        .send(Message::Binary(
            rmp_serde::to_vec_named(&battery_write("binary-request", 54))
                .unwrap()
                .into(),
        ))
        .await
        .unwrap();
    assert!(matches!(
        recv_messagepack(&mut socket).await,
        ServerMessage::Reply { request_id, .. } if request_id.as_str() == "binary-request"
    ));
    assert!(matches!(
        recv_json(&mut socket).await,
        ServerMessage::Update { .. }
    ));

    socket.close(None).await.unwrap();
    wait_for_session_count(&server.core, 0).await;
    server.stop().await;
}

#[tokio::test]
async fn websocket_core_errors_use_the_common_code_and_request_id() {
    let server = TestServer::start().await;
    let mut socket = server.connect().await;
    send_hello(&mut socket, "client", &[]).await;
    let _ = recv_json(&mut socket).await;

    send_json(
        &mut socket,
        json!({
            "type": "write",
            "request_id": "forbidden-write",
            "operations": [{
                "op": "publish_state",
                "topic": "/$connections/fake",
                "value": true,
                "expiry": {"mode": "clear"}
            }]
        }),
    )
    .await;
    assert!(matches!(
        recv_json(&mut socket).await,
        ServerMessage::Error { request_id: Some(request_id), error }
            if request_id.as_str() == "forbidden-write" && error.code == "system_topic"
    ));

    socket.close(None).await.unwrap();
    wait_for_session_count(&server.core, 0).await;
    server.stop().await;
}

async fn recv_messagepack(socket: &mut TestSocket) -> ServerMessage {
    let message = timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("server response timed out")
        .expect("WebSocket closed")
        .expect("WebSocket read failed");
    match message {
        Message::Binary(bytes) => rmp_serde::from_slice(&bytes).unwrap(),
        other => panic!("expected binary message, received {other:?}"),
    }
}

async fn wait_for_session_count(core: &SharedCore, expected: usize) {
    timeout(Duration::from_secs(2), async {
        loop {
            if core.lock().unwrap().managed_session_count() == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("managed session cleanup timed out");
}

#[tokio::test]
async fn disconnect_immediately_releases_an_owned_input_claim() {
    let server = TestServer::start().await;
    let mut socket = server.connect().await;
    send_hello(&mut socket, "lamp controller", &["/lamp/desired"]).await;
    let _ = recv_json(&mut socket).await;
    send_json(
        &mut socket,
        json!({
            "type": "write",
            "request_id": "claim",
            "operations": [
                {"op": "define_input", "topic": "/lamp/desired", "kind": "desired"},
                {"op": "claim_input", "topic": "/lamp/desired", "release": {"mode": "immediate"}}
            ]
        }),
    )
    .await;
    let _ = recv_json(&mut socket).await;
    let _ = recv_json(&mut socket).await;
    socket.close(None).await.unwrap();
    wait_for_session_count(&server.core, 0).await;

    let snapshot = server.core.lock().unwrap().read(&Selection::new(vec![
        Selector::parse("/lamp/desired").unwrap(),
    ]));
    let node = snapshot
        .nodes()
        .get(&tanuki::domain::TopicPath::parse("/lamp/desired").unwrap())
        .unwrap();
    let Node::Desired(node) = node else {
        panic!("lamp input must remain a desired node");
    };
    assert!(node.claim().is_none());
    server.stop().await;
}

#[tokio::test]
async fn simulated_room_actors_drive_downstream_outputs_across_transports() {
    let server = TestServer::start().await;

    let mut dashboard = server.connect().await;
    send_hello(
        &mut dashboard,
        "dashboard",
        &["/battery/*", "/lamp/*", "/location"],
    )
    .await;
    let _ = recv_json(&mut dashboard).await;

    let mut laptop = server.connect().await;
    send_hello(&mut laptop, "laptop", &[]).await;
    let _ = recv_json(&mut laptop).await;
    send_json(&mut laptop, battery_write("laptop-battery", 82)).await;
    let _ = recv_json(&mut laptop).await;
    assert_update_topics(&mut dashboard, &["/battery/laptop"]).await;

    let phone = server
        .post_json(
            "/v1/state/battery/phone",
            "phone task",
            json!({"value": 63, "expiry": {"mode": "clear"}}),
        )
        .await;
    assert_eq!(phone["ok"], true);
    assert_update_topics(&mut dashboard, &["/battery/phone"]).await;

    let mut controller = server.connect().await;
    send_hello(&mut controller, "lamp controller", &["/lamp/desired"]).await;
    let _ = recv_json(&mut controller).await;
    send_json(
        &mut controller,
        json!({
            "type": "write",
            "request_id": "own-lamp",
            "operations": [
                {"op": "define_input", "topic": "/lamp/desired", "kind": "desired"},
                {"op": "claim_input", "topic": "/lamp/desired", "release": {"mode": "immediate"}}
            ]
        }),
    )
    .await;
    assert!(matches!(
        recv_json(&mut controller).await,
        ServerMessage::Reply { request_id, .. } if request_id.as_str() == "own-lamp"
    ));
    assert_update_topics(&mut controller, &["/lamp/desired"]).await;
    assert_update_topics(&mut dashboard, &["/lamp/desired"]).await;

    let remote = server
        .post_json(
            "/v1/write",
            "remote",
            json!({"operations": [{
                "op": "submit_desired",
                "topic": "/lamp/desired",
                "value": 70,
                "expiry": {"mode": "clear"}
            }]}),
        )
        .await;
    assert_eq!(remote["ok"], true);
    assert_update_topics(&mut controller, &["/lamp/desired"]).await;
    assert_update_topics(&mut dashboard, &["/lamp/desired"]).await;

    send_json(
        &mut controller,
        json!({
            "type": "write",
            "request_id": "lamp-actual",
            "operations": [
                {"op": "publish_state", "topic": "/lamp/hue", "value": 35, "expiry": {"mode": "clear"}},
                {"op": "publish_state", "topic": "/lamp/brightness", "value": 70, "expiry": {"mode": "clear"}}
            ]
        }),
    )
    .await;
    assert!(matches!(
        recv_json(&mut controller).await,
        ServerMessage::Reply { request_id, .. } if request_id.as_str() == "lamp-actual"
    ));
    assert_update_topics(&mut dashboard, &["/lamp/brightness", "/lamp/hue"]).await;

    let mut location_script = server.connect().await;
    send_hello(&mut location_script, "location script", &["/motion/*"]).await;
    let _ = recv_json(&mut location_script).await;
    let mut automation = server.connect().await;
    send_hello(&mut automation, "room automation", &["/location"]).await;
    let _ = recv_json(&mut automation).await;

    let motion = server
        .post_json(
            "/v1/state/motion/kitchen",
            "kitchen sensor",
            json!({"value": true, "expiry": {"mode": "clear"}}),
        )
        .await;
    assert_eq!(motion["ok"], true);
    assert_update_topics(&mut location_script, &["/motion/kitchen"]).await;

    send_json(
        &mut location_script,
        json!({
            "type": "write",
            "request_id": "infer-location",
            "operations": [{
                "op": "publish_state",
                "topic": "/location",
                "value": "kitchen",
                "expiry": {"mode": "clear"}
            }]
        }),
    )
    .await;
    let _ = recv_json(&mut location_script).await;
    assert_update_topics(&mut automation, &["/location"]).await;
    assert_update_topics(&mut dashboard, &["/location"]).await;

    send_json(
        &mut automation,
        json!({
            "type": "write",
            "request_id": "location-light",
            "operations": [{
                "op": "submit_desired",
                "topic": "/lamp/desired",
                "value": 90,
                "expiry": {"mode": "clear"}
            }]
        }),
    )
    .await;
    let _ = recv_json(&mut automation).await;
    assert_update_topics(&mut controller, &["/lamp/desired"]).await;
    assert_update_topics(&mut dashboard, &["/lamp/desired"]).await;

    for socket in [
        &mut automation,
        &mut location_script,
        &mut controller,
        &mut laptop,
        &mut dashboard,
    ] {
        socket.close(None).await.unwrap();
    }
    wait_for_session_count(&server.core, 0).await;
    server.stop().await;
}

#[tokio::test(start_paused = true)]
async fn explicit_expiry_and_disconnect_grace_flow_through_live_transports() {
    let second = Arc::new(AtomicI64::new(0));
    let clock_second = Arc::clone(&second);
    let clock: Clock = Arc::new(move || {
        Timestamp::new(JiffTimestamp::from_second(clock_second.load(Ordering::SeqCst)).unwrap())
    });
    let server = TestServer::start_with_clock(clock).await;

    let mut dashboard = server.connect().await;
    send_hello(&mut dashboard, "dashboard", &["/lamp/*", "/battery/*"]).await;
    let _ = recv_json(&mut dashboard).await;
    let mut controller = server.connect().await;
    send_hello(&mut controller, "controller", &[]).await;
    let _ = recv_json(&mut controller).await;
    send_json(
        &mut controller,
        json!({
            "type": "write",
            "request_id": "claim-with-grace",
            "operations": [
                {"op": "define_input", "topic": "/lamp/desired", "kind": "desired"},
                {"op": "claim_input", "topic": "/lamp/desired", "release": {"mode": "after", "duration": "PT10S"}}
            ]
        }),
    )
    .await;
    assert!(matches!(
        recv_json(&mut controller).await,
        ServerMessage::Reply { request_id, .. } if request_id.as_str() == "claim-with-grace"
    ));
    tokio::task::yield_now().await;
    assert_update_topics_without_timeout(&mut dashboard, &["/lamp/desired"]).await;
    controller.close(None).await.unwrap();
    wait_for_session_count(&server.core, 1).await;

    second.store(5, Ordering::SeqCst);
    let submitted = server
        .post_json(
            "/v1/write",
            "remote",
            json!({"operations": [{
                "op": "submit_desired",
                "topic": "/lamp/desired",
                "value": 80,
                "expiry": {"mode": "clear"}
            }]}),
        )
        .await;
    assert_eq!(submitted["ok"], true);
    assert_update_topics(&mut dashboard, &["/lamp/desired"]).await;

    second.store(10, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(10)).await;
    assert_update_topics(&mut dashboard, &["/lamp/desired"]).await;
    let snapshot = server.core.lock().unwrap().read(&Selection::new(vec![
        Selector::parse("/lamp/desired").unwrap(),
    ]));
    let Node::Desired(desired) = snapshot
        .nodes()
        .get(&TopicPath::parse("/lamp/desired").unwrap())
        .unwrap()
    else {
        panic!("desired definition must survive grace release");
    };
    assert!(desired.claim().is_none());
    assert_eq!(desired.current().unwrap().value(), &Value::Integer(80));

    let expiring = server
        .post_json(
            "/v1/state/battery/phone",
            "phone",
            json!({"value": 55, "expiry": {"mode": "set", "duration": "PT5S"}}),
        )
        .await;
    assert_eq!(expiring["ok"], true);
    assert_update_topics(&mut dashboard, &["/battery/phone"]).await;
    second.store(15, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(5)).await;
    assert_update_topics(&mut dashboard, &["/battery/phone"]).await;
    assert!(
        !server
            .core
            .lock()
            .unwrap()
            .read(&Selection::new(vec![
                Selector::parse("/battery/phone").unwrap()
            ]))
            .nodes()
            .contains_key(&TopicPath::parse("/battery/phone").unwrap())
    );

    dashboard.close(None).await.unwrap();
    wait_for_session_count(&server.core, 0).await;
    server.stop().await;
}

async fn assert_update_topics(socket: &mut TestSocket, expected: &[&str]) {
    let ServerMessage::Update { changes, .. } = recv_json(socket).await else {
        panic!("expected an update message");
    };
    assert_topics(&changes, expected);
}

async fn assert_update_topics_without_timeout(socket: &mut TestSocket, expected: &[&str]) {
    let message = socket
        .next()
        .await
        .expect("WebSocket closed")
        .expect("WebSocket read failed");
    let Message::Text(text) = message else {
        panic!("expected text update, received {message:?}");
    };
    let ServerMessage::Update { changes, .. } = serde_json::from_str(&text).unwrap() else {
        panic!("expected an update message");
    };
    assert_topics(&changes, expected);
}

fn assert_topics(changes: &[tanuki::protocol::ChangeView], expected: &[&str]) {
    let mut topics = changes
        .iter()
        .map(|change| match change {
            tanuki::protocol::ChangeView::Upsert { topic, .. }
            | tanuki::protocol::ChangeView::Event { topic, .. }
            | tanuki::protocol::ChangeView::Command { topic, .. }
            | tanuki::protocol::ChangeView::Removed { topic, .. } => topic.as_str(),
        })
        .collect::<Vec<_>>();
    topics.sort_unstable();
    assert_eq!(topics, expected);
}

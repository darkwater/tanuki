use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use jiff::Timestamp as JiffTimestamp;
use serde_json::{Value as RawJson, json};
use tanuki::{
    core::{Core, SchemaInstallMode},
    domain::{Node, Selection, Selector, Timestamp, TopicPath, Value},
    link::{LinkDefinition, LinkName},
    protocol::{DiagnosticView, NodeView, ServerMessage, WireValue},
    schema::{Enforcement, NullPolicy, Schema, SchemaName, SchemaRule, ValueValidator},
    server::serve_with_core_and_timeouts,
    transport::{Clock, Heartbeat, SharedCore, WebSocketTimeouts},
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

    /// No liveness bounds: paused-clock tests auto-advance virtual time to the
    /// next timer during real socket I/O, which would fire any liveness timer.
    async fn start_with_clock(clock: Clock) -> Self {
        Self::start_with(clock, WebSocketTimeouts::DISABLED).await
    }

    async fn start_with_timeouts(timeouts: WebSocketTimeouts) -> Self {
        let clock: Clock =
            Arc::new(|| Timestamp::new(JiffTimestamp::from_second(1_700_000_000).unwrap()));
        Self::start_with(clock, timeouts).await
    }

    async fn start_with(clock: Clock, timeouts: WebSocketTimeouts) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let core = Arc::new(Mutex::new(Core::new()));
        let (shutdown, stopped) = oneshot::channel();
        let task = tokio::spawn(serve_with_core_and_timeouts(
            listener,
            core.clone(),
            clock,
            timeouts,
            async move {
                let _ = stopped.await;
            },
        ));
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

/// Next data message. Server pings are liveness plumbing and are skipped;
/// reading them lets tungstenite queue the pong reply.
async fn next_data(socket: &mut TestSocket) -> Message {
    loop {
        let message = socket
            .next()
            .await
            .expect("WebSocket closed")
            .expect("WebSocket read failed");
        if !matches!(message, Message::Ping(_) | Message::Pong(_)) {
            return message;
        }
    }
}

async fn recv_json(socket: &mut TestSocket) -> ServerMessage {
    let message = timeout(Duration::from_secs(2), next_data(socket))
        .await
        .expect("server response timed out");
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
            if request_id.as_str() == "invalid" && error.code == tanuki::protocol::ErrorCode::SchemaViolation
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
    assert_update_topics(
        &mut socket,
        &["/devices/lamp/power", "/rooms/living-room/power"],
    )
    .await;
    assert!(matches!(
        recv_json(&mut socket).await,
        ServerMessage::Reply { request_id, .. } if request_id.as_str() == "alias-write"
    ));

    socket.close(None).await.unwrap();
    wait_for_session_count(&server.core, 0).await;
    server.stop().await;
}

#[tokio::test]
async fn json_hello_snapshot_then_own_atomic_update_then_write_reply() {
    let server = TestServer::start().await;
    let mut socket = server.connect().await;
    send_hello(&mut socket, "laptop", &["/battery/*"]).await;

    assert!(matches!(
        recv_json(&mut socket).await,
        ServerMessage::Snapshot { request_id, ref nodes, .. }
            if request_id.as_str() == "hello-1" && nodes.is_empty()
    ));

    send_json(&mut socket, battery_write("write-1", 87)).await;
    // The writer's own selected update precedes its reply, so a client's cache
    // already reflects the write when the reply arrives.
    let ServerMessage::Update { changes, sequence } = recv_json(&mut socket).await else {
        panic!("the committed update must precede its correlated reply");
    };
    assert_eq!(changes.len(), 1);
    let ServerMessage::Reply { request_id, result } = recv_json(&mut socket).await else {
        panic!("the correlated reply must follow the update");
    };
    assert_eq!(request_id.as_str(), "write-1");
    assert_eq!(result["sequence"], sequence);

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
    // The stateless HTTP write's update, then this write's update, then its reply.
    assert!(matches!(
        recv_json(&mut original).await,
        ServerMessage::Update { .. }
    ));
    assert!(matches!(
        recv_json(&mut original).await,
        ServerMessage::Update { .. }
    ));
    assert!(matches!(
        recv_json(&mut original).await,
        ServerMessage::Reply { request_id, .. } if request_id.as_str() == "still-current"
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
        ServerMessage::Update { .. }
    ));
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
    let ServerMessage::Update { changes, .. } = recv_messagepack(&mut socket).await else {
        panic!("binary subscriber must receive the committed update");
    };
    assert!(matches!(
        recv_messagepack(&mut socket).await,
        ServerMessage::Reply { request_id, .. } if request_id.as_str() == "binary-write"
    ));
    let NodeView::State { current } = (match changes.as_slice() {
        [tanuki::protocol::ChangeView::Upsert { node, .. }] => node,
        other => panic!("unexpected changes: {other:?}"),
    }) else {
        panic!("battery update must be state");
    };
    assert_eq!(current.value, WireValue::new(Value::Integer(91)));

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
    // Unsolicited updates keep the hello codec; the reply uses the request's.
    assert!(matches!(
        recv_json(&mut socket).await,
        ServerMessage::Update { .. }
    ));
    assert!(matches!(
        recv_messagepack(&mut socket).await,
        ServerMessage::Reply { request_id, .. } if request_id.as_str() == "binary-request"
    ));

    socket.close(None).await.unwrap();
    wait_for_session_count(&server.core, 0).await;
    server.stop().await;
}

#[tokio::test]
async fn trailing_messagepack_data_rejects_the_entire_write_without_mutation() {
    let server = TestServer::start().await;
    let mut socket = server.connect().await;
    send_hello(&mut socket, "framing probe", &["/battery/*"]).await;
    let _ = recv_json(&mut socket).await;
    let selection = Selection::new(vec![Selector::parse("/battery/*").unwrap()]);
    let before = server.core.lock().unwrap().read(&selection);

    let mut bytes = rmp_serde::to_vec_named(&battery_write("invalid-frame", 54)).unwrap();
    bytes.push(0xc0);
    socket.send(Message::Binary(bytes.into())).await.unwrap();
    assert!(matches!(
        recv_messagepack(&mut socket).await,
        ServerMessage::Error { request_id: None, error } if error.code == tanuki::protocol::ErrorCode::InvalidMessage
    ));
    let after = server.core.lock().unwrap().read(&selection);
    assert_eq!(after.sequence(), before.sequence());
    assert!(after.nodes().is_empty());

    // A malformed request does not prevent a subsequent complete request.
    send_json(&mut socket, battery_write("valid-frame", 55)).await;
    assert!(matches!(
        recv_json(&mut socket).await,
        ServerMessage::Update { .. }
    ));
    assert!(matches!(
        recv_json(&mut socket).await,
        ServerMessage::Reply { .. }
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
            if request_id.as_str() == "forbidden-write" && error.code == tanuki::protocol::ErrorCode::SystemTopic
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
        &[
            "/battery/*",
            "/lamp/*",
            "/location",
            "/dashboard/**",
            "/$diagnostics/**",
        ],
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
    assert_update_topics(&mut controller, &["/lamp/desired"]).await;
    assert!(matches!(
        recv_json(&mut controller).await,
        ServerMessage::Reply { request_id, .. } if request_id.as_str() == "own-lamp"
    ));
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

    server
        .core
        .lock()
        .unwrap()
        .install_schema(
            Schema::new(
                SchemaName::parse("battery views").unwrap(),
                vec![
                    SchemaRule::new(
                        Selector::parse("/battery/*").unwrap(),
                        Enforcement::Warn,
                        ValueValidator::integer_range(0, 100, NullPolicy::Deny).unwrap(),
                    )
                    .unwrap(),
                    SchemaRule::new(
                        Selector::parse("/dashboard/battery/*").unwrap(),
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
    server
        .core
        .lock()
        .unwrap()
        .install_link(
            LinkDefinition::new(
                LinkName::parse("dashboard-battery").unwrap(),
                TopicPath::parse("/dashboard/battery").unwrap(),
                TopicPath::parse("/battery").unwrap(),
            )
            .unwrap(),
            Timestamp::new(JiffTimestamp::from_second(1_700_000_000).unwrap()),
        )
        .unwrap();
    assert_update_topics(
        &mut dashboard,
        &["/dashboard/battery/laptop", "/dashboard/battery/phone"],
    )
    .await;

    let unusual = server
        .post_json(
            "/v1/state/battery/phone",
            "phone task",
            json!({"value": 101, "expiry": {"mode": "clear"}}),
        )
        .await;
    assert_eq!(unusual["ok"], true);
    assert_eq!(unusual["data"]["warnings"][0]["code"], "schema_warning");
    assert_eq!(unusual["data"]["warnings"][1]["code"], "link_disabled");
    assert_update_topics(
        &mut dashboard,
        &[
            "/$diagnostics/links/dashboard-battery",
            "/battery/phone",
            "/dashboard/battery/laptop",
            "/dashboard/battery/phone",
        ],
    )
    .await;

    let repaired = server
        .post_json(
            "/v1/state/battery/phone",
            "phone task",
            json!({"value": 60, "expiry": {"mode": "clear"}}),
        )
        .await;
    assert_eq!(repaired["ok"], true);
    assert_eq!(repaired["data"]["warnings"][0]["code"], "link_enabled");
    assert_update_topics(
        &mut dashboard,
        &[
            "/$diagnostics/links/dashboard-battery",
            "/battery/phone",
            "/dashboard/battery/laptop",
            "/dashboard/battery/phone",
        ],
    )
    .await;

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
    let message = next_data(socket).await;
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
            | tanuki::protocol::ChangeView::Removed { topic, .. } => topic.to_string(),
        })
        .collect::<Vec<_>>();
    topics.sort_unstable();
    assert_eq!(topics, expected);
}

fn quick_timeouts() -> WebSocketTimeouts {
    WebSocketTimeouts {
        hello: Some(Duration::from_millis(200)),
        heartbeat: Some(Heartbeat {
            interval: Duration::from_millis(50),
            silence: Duration::from_millis(300),
        }),
        send: Some(Duration::from_secs(1)),
    }
}

async fn expect_close(socket: &mut TestSocket, expected: CloseCode) {
    timeout(Duration::from_secs(2), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(Some(frame)))) => {
                    assert_eq!(frame.code, expected, "close reason: {}", frame.reason);
                    return;
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                other => panic!("expected close {expected:?}, received {other:?}"),
            }
        }
    })
    .await
    .expect("server did not close the WebSocket");
}

#[tokio::test]
async fn a_peer_that_never_sends_hello_is_closed() {
    let server = TestServer::start_with_timeouts(quick_timeouts()).await;
    let mut socket = server.connect().await;
    expect_close(&mut socket, CloseCode::Policy).await;
    server.stop().await;
}

#[tokio::test]
async fn a_silent_peer_loses_its_session_and_input_claim() {
    let server = TestServer::start_with_timeouts(quick_timeouts()).await;
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

    // Stop polling: tungstenite answers pings only while the stream is read,
    // which models a peer that vanished without closing TCP.
    wait_for_session_count(&server.core, 0).await;
    let snapshot = server.core.lock().unwrap().read(&Selection::new(vec![
        Selector::parse("/lamp/desired").unwrap(),
    ]));
    let Some(Node::Desired(node)) = snapshot
        .nodes()
        .get(&TopicPath::parse("/lamp/desired").unwrap())
    else {
        panic!("lamp input must remain a desired node");
    };
    assert!(node.claim().is_none());
    drop(socket);
    server.stop().await;
}

#[tokio::test]
async fn a_responsive_idle_peer_stays_connected_past_the_silence_window() {
    let server = TestServer::start_with_timeouts(quick_timeouts()).await;
    let mut socket = server.connect().await;
    send_hello(&mut socket, "quiet dashboard", &[]).await;
    let _ = recv_json(&mut socket).await;

    // Reading lets the client library answer each server ping with a pong.
    let mut pings = 0;
    let idle = timeout(Duration::from_millis(900), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Ping(_))) => pings += 1,
                other => panic!("idle session must only see pings, received {other:?}"),
            }
        }
    })
    .await;

    assert!(idle.is_err(), "the idle session must stay open");
    assert!(pings >= 3, "expected periodic server pings, saw {pings}");
    assert_eq!(server.core.lock().unwrap().managed_session_count(), 1);
    server.stop().await;
}

#[tokio::test]
async fn a_snapshot_too_big_for_one_message_closes_with_message_too_big() {
    use tanuki::{
        core::MAX_VALUE_JSON_BYTES,
        domain::{ClientName, ExpiryUpdate, WriteBatch, WriteContext, WriteOperation},
    };
    let server = TestServer::start().await;
    // Each value is individually acceptable; together they exceed 1 MiB.
    for index in 0..5 {
        server
            .core
            .lock()
            .unwrap()
            .apply(
                &WriteContext::stateless(ClientName::parse("uploader").unwrap()),
                WriteBatch::new(vec![WriteOperation::PublishState {
                    topic: TopicPath::parse(&format!("/blob/{index}")).unwrap(),
                    value: Value::String("x".repeat(MAX_VALUE_JSON_BYTES - 2)),
                    expiry: ExpiryUpdate::Preserve,
                }])
                .unwrap(),
                Timestamp::new(JiffTimestamp::from_second(1_700_000_000).unwrap()),
            )
            .unwrap();
    }

    let mut socket = server.connect().await;
    send_hello(&mut socket, "blob reader", &["/blob/*"]).await;
    expect_close(&mut socket, CloseCode::Size).await;
    wait_for_session_count(&server.core, 0).await;
    server.stop().await;
}

async fn send_stateless_hello(socket: &mut TestSocket, client: &str, selectors: &[&str]) {
    send_json(
        socket,
        json!({
            "type": "hello",
            "request_id": "hello-1",
            "client": client,
            "selectors": selectors,
            "session": "stateless",
        }),
    )
    .await;
}

#[tokio::test]
async fn stateless_sessions_neither_replace_nor_claim_but_attribute_writes() {
    let server = TestServer::start().await;
    let mut managed = server.connect().await;
    send_hello(&mut managed, "dashboard", &["/lamp/*"]).await;
    let _ = recv_json(&mut managed).await;
    let mut tab_one = server.connect().await;
    send_stateless_hello(&mut tab_one, "dashboard", &["/lamp/*"]).await;
    assert!(matches!(
        recv_json(&mut tab_one).await,
        ServerMessage::Snapshot { ref warnings, .. } if warnings.is_empty()
    ));
    let mut tab_two = server.connect().await;
    send_stateless_hello(&mut tab_two, "dashboard", &["/lamp/*"]).await;
    let _ = recv_json(&mut tab_two).await;
    assert_eq!(server.core.lock().unwrap().managed_session_count(), 1);

    send_json(
        &mut tab_one,
        json!({"type": "write", "request_id": "level", "operations": [
            {"op": "publish_state", "topic": "/lamp/level", "value": 5}
        ]}),
    )
    .await;
    let ServerMessage::Update { changes, .. } = recv_json(&mut tab_one).await else {
        panic!("the stateless writer still observes its own update first");
    };
    let [
        tanuki::protocol::ChangeView::Upsert {
            node: NodeView::State { current },
            ..
        },
    ] = changes.as_slice()
    else {
        panic!("unexpected changes: {changes:?}");
    };
    assert_eq!(current.last_write.client, "dashboard");
    assert_eq!(current.last_write.session, None);
    assert!(matches!(
        recv_json(&mut tab_one).await,
        ServerMessage::Reply { .. }
    ));
    // Neither the managed connection nor the other tab was displaced.
    assert_update_topics(&mut managed, &["/lamp/level"]).await;
    assert_update_topics(&mut tab_two, &["/lamp/level"]).await;

    send_json(
        &mut tab_one,
        json!({"type": "write", "request_id": "claim", "operations": [
            {"op": "define_input", "topic": "/lamp/desired", "kind": "desired"},
            {"op": "claim_input", "topic": "/lamp/desired", "release": {"mode": "immediate"}}
        ]}),
    )
    .await;
    assert!(matches!(
        recv_json(&mut tab_one).await,
        ServerMessage::Error { request_id: Some(id), error }
            if id.as_str() == "claim" && error.code == tanuki::protocol::ErrorCode::SessionRequired
    ));
    for socket in [&mut managed, &mut tab_one, &mut tab_two] {
        socket.close(None).await.unwrap();
    }
    wait_for_session_count(&server.core, 0).await;
    server.stop().await;
}

async fn post_state(server: &TestServer, topic: &str, value: i64) -> RawJson {
    server
        .post_json(
            &format!("/v1/state{topic}"),
            "phone",
            json!({"value": value}),
        )
        .await
}

#[tokio::test]
async fn select_replaces_the_selection_with_a_correlated_snapshot() {
    let server = TestServer::start().await;
    let mut socket = server.connect().await;
    send_hello(&mut socket, "viewer", &[]).await;
    let _ = recv_json(&mut socket).await;
    assert_eq!(post_state(&server, "/battery/phone", 50).await["ok"], true);

    send_json(
        &mut socket,
        json!({"type": "select", "request_id": "sel-1", "selectors": ["/battery/*"]}),
    )
    .await;
    let ServerMessage::Snapshot {
        request_id, nodes, ..
    } = recv_json(&mut socket).await
    else {
        panic!("select must be answered by a snapshot");
    };
    assert_eq!(request_id.as_str(), "sel-1");
    assert_eq!(
        nodes.keys().map(ToString::to_string).collect::<Vec<_>>(),
        ["/battery/phone"]
    );

    assert_eq!(post_state(&server, "/battery/laptop", 60).await["ok"], true);
    assert_update_topics(&mut socket, &["/battery/laptop"]).await;

    send_json(
        &mut socket,
        json!({"type": "select", "request_id": "sel-2", "selectors": []}),
    )
    .await;
    assert!(matches!(
        recv_json(&mut socket).await,
        ServerMessage::Snapshot { ref nodes, .. } if nodes.is_empty()
    ));
    assert_eq!(post_state(&server, "/battery/laptop", 61).await["ok"], true);
    assert!(
        timeout(Duration::from_millis(200), next_data(&mut socket))
            .await
            .is_err(),
        "an emptied selection must not receive updates"
    );
    socket.close(None).await.unwrap();
    wait_for_session_count(&server.core, 0).await;
    server.stop().await;
}

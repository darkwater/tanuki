use std::sync::{Arc, Mutex};
use tanuki::{core::Core, domain::Timestamp, server::serve_with_core, transport::SharedCore};
use tanuki_client::protocol::*;
use tanuki_client::{Codec, Connection};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

struct Server {
    url: String,
    core: SharedCore,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}
impl Server {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/v1/ws", listener.local_addr().unwrap());
        let core = Arc::new(Mutex::new(Core::new()));
        let (shutdown, rx) = oneshot::channel();
        let task = tokio::spawn(serve_with_core(
            listener,
            core.clone(),
            Arc::new(|| Timestamp::new(jiff::Timestamp::from_second(1700000000).unwrap())),
            async {
                let _ = rx.await;
            },
        ));
        Self {
            url,
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
fn hello(name: &str, selectors: &[&str]) -> ClientMessage {
    ClientMessage::Hello {
        request_id: RequestId::new("hello".into()),
        client: ClientName::parse(name).unwrap(),
        selectors: selectors
            .iter()
            .map(|s| Selector::parse(s).unwrap())
            .collect(),
    }
}
fn publish(value: i64) -> WireOperation {
    WireOperation::PublishState {
        topic: TopicPath::parse("/battery/laptop").unwrap(),
        value: JsonValue::new(Value::Integer(value)),
        expiry: Some(WireExpiry::Clear),
    }
}
#[tokio::test]
async fn direct_connection_receives_hello_reply_update_and_remote_error_in_both_codecs() {
    for codec in [Codec::Json, Codec::MessagePack] {
        let server = Server::start().await;
        let mut connection = Connection::open_with_codec(&server.url, codec)
            .await
            .unwrap();
        connection
            .send(&hello("laptop", &["/battery/*"]))
            .await
            .unwrap();
        assert!(
            matches!(receive(connection.recv()).await.unwrap(), Some(ServerMessage::Snapshot { nodes, .. }) if nodes.is_empty())
        );
        connection
            .send_with_codec(
                &ClientMessage::Write {
                    request_id: RequestId::new("write".into()),
                    operations: vec![publish(82)],
                },
                if codec == Codec::Json {
                    Codec::MessagePack
                } else {
                    Codec::Json
                },
            )
            .await
            .unwrap();
        assert!(
            matches!(receive(connection.recv()).await.unwrap(), Some(ServerMessage::Reply { request_id, .. }) if request_id.as_str() == "write")
        );
        assert!(
            matches!(receive(connection.recv()).await.unwrap(), Some(ServerMessage::Update { changes, .. }) if changes.len() == 1)
        );
        connection
            .send(&ClientMessage::Write {
                request_id: RequestId::new("invalid".into()),
                operations: vec![],
            })
            .await
            .unwrap();
        assert!(
            matches!(receive(connection.recv()).await.unwrap(), Some(ServerMessage::Error { request_id: Some(id), error }) if id.as_str() == "invalid" && error.code == "empty_batch")
        );
        connection.close().await.unwrap();
        server.stop().await;
    }
}

#[tokio::test]
async fn session_writes_without_listener_polling_and_registers_current_baselines() {
    use tanuki_client::Session;
    let server = Server::start().await;
    let producer = Session::connect(&server.url, ClientName::parse("producer").unwrap(), vec![])
        .await
        .unwrap();
    let dashboard = Session::connect(
        &server.url,
        ClientName::parse("dashboard").unwrap(),
        vec![Selector::parse("/battery/*").unwrap()],
    )
    .await
    .unwrap();
    let receipt = producer.write(vec![publish(80)]).await.unwrap();
    assert!(receipt.warnings.is_empty());
    let mut first = dashboard.listen_updates().await.unwrap();
    // The baseline or the next delta contains the concurrent committed value, never neither.
    let mut view = tanuki_client::SelectedView::from_snapshot(first.initial_snapshot().clone());
    if view.nodes().is_empty() {
        view.apply(receive(first.recv()).await.unwrap().unwrap())
            .unwrap();
    }
    assert_eq!(view.nodes().len(), 1);
    let mut second = dashboard.listen_updates().await.unwrap();
    assert_eq!(second.initial_snapshot().nodes, *view.nodes());
    producer.write(vec![publish(81)]).await.unwrap();
    let a = receive(first.recv()).await.unwrap().unwrap();
    let b = receive(second.recv()).await.unwrap().unwrap();
    assert_eq!(a, b);
    assert_eq!(server.core.lock().unwrap().managed_session_count(), 2);
    dashboard.close().await.unwrap();
    producer.close().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn typed_lamp_handles_keep_claims_explicit_and_batch_rejection_atomic() {
    use tanuki_client::{ClaimRelease, ClientError, ExpiryUpdate, Session};
    let server = Server::start().await;
    let owner = Session::connect(
        &server.url,
        ClientName::parse("controller").unwrap(),
        vec![Selector::parse("/lamp/**").unwrap()],
    )
    .await
    .unwrap();
    let remote = Session::connect(&server.url, ClientName::parse("remote").unwrap(), vec![])
        .await
        .unwrap();
    let desired = owner.desired::<u8>("/lamp/desired-brightness").unwrap();
    assert!(
        server
            .core
            .lock()
            .unwrap()
            .read(&Selection::new(vec![Selector::parse("/lamp/**").unwrap()]))
            .nodes()
            .is_empty()
    );
    let mut setup = owner.batch();
    setup.define(&desired).unwrap();
    setup.claim(&desired, ClaimRelease::Immediate).unwrap();
    setup.commit().await.unwrap();
    let mut raw = owner.listen_updates().await.unwrap();
    let mut view = tanuki_client::SelectedView::from_snapshot(raw.initial_snapshot().clone());
    if view.nodes().is_empty() {
        view.apply(receive(raw.recv()).await.unwrap().unwrap())
            .unwrap();
    }
    let NodeView::Desired { claim: before, .. } =
        view.nodes().get("/lamp/desired-brightness").unwrap()
    else {
        panic!()
    };
    let before = before.clone();
    assert!(before.is_some());
    remote
        .desired::<u8>("/lamp/desired-brightness")
        .unwrap()
        .submit(&70, ExpiryUpdate::Clear)
        .await
        .unwrap();
    view.apply(receive(raw.recv()).await.unwrap().unwrap())
        .unwrap();
    let NodeView::Desired {
        claim: after,
        current,
        ..
    } = view.nodes().get("/lamp/desired-brightness").unwrap()
    else {
        panic!()
    };
    assert_eq!(&before, after);
    assert!(current.is_some());
    assert_eq!(
        desired
            .decode(view.nodes().get("/lamp/desired-brightness").unwrap())
            .unwrap()
            .current()
            .unwrap()
            .value,
        70
    );
    let hue = owner.state::<u16>("/lamp/hue").unwrap();
    let brightness = owner.state::<u8>("/lamp/brightness").unwrap();
    let mut batch = owner.batch();
    batch.publish(&hue, &120, ExpiryUpdate::Clear).unwrap();
    batch
        .publish(&brightness, &70, ExpiryUpdate::Clear)
        .unwrap();
    batch.commit().await.unwrap();
    let paired = receive(raw.recv()).await.unwrap().unwrap();
    assert_eq!(paired.changes.len(), 2);
    let foreign = remote.state::<u16>("/lamp/hue").unwrap();
    assert!(matches!(
        owner.batch().publish(&foreign, &35, ExpiryUpdate::Clear),
        Err(ClientError::WrongSession)
    ));
    let event = owner.event::<u8>("/lamp/event").unwrap();
    let undefined = owner.command::<u8>("/lamp/undefined").unwrap();
    let mut invalid = owner.batch();
    invalid
        .publish(&brightness, &99, ExpiryUpdate::Clear)
        .unwrap();
    invalid.emit(&event, &1).unwrap();
    invalid.command(&undefined, &1).unwrap();
    assert!(matches!(
        invalid.commit().await,
        Err(ClientError::Rejected(_))
    ));
    let snapshot = server
        .core
        .lock()
        .unwrap()
        .read(&Selection::new(vec![Selector::parse("/lamp/**").unwrap()]));
    assert!(!snapshot.nodes().contains_key(event.path()));
    assert_eq!(
        snapshot.nodes()[brightness.path()]
            .retained_value()
            .unwrap()
            .value(),
        &Value::Integer(70)
    );
    let keep = hue.clone();
    drop(owner);
    assert!(matches!(
        keep.publish(&10, ExpiryUpdate::Clear).await,
        Err(ClientError::SessionGone)
    ));
    remote.close().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn observers_deliver_initial_empty_then_atomic_complete_immutable_snapshots() {
    use tanuki_client::{ClientError, ExpiryUpdate, Session};
    let server = Server::start().await;
    let session = Session::connect(
        &server.url,
        ClientName::parse("lamp dashboard").unwrap(),
        vec![Selector::parse("/lamp/**").unwrap()],
    )
    .await
    .unwrap();
    let hue = session.state::<u16>("/lamp/hue").unwrap();
    let brightness = session.state::<u8>("/lamp/brightness").unwrap();
    let mut observed = session
        .observe_batch([
            hue.path().clone(),
            brightness.path().clone(),
            hue.path().clone(),
        ])
        .await
        .unwrap();
    let initial = receive(observed.recv()).await.unwrap().unwrap();
    assert!(initial.nodes().is_empty());
    assert_eq!(initial.paths().len(), 2);
    let omitted = session.state::<u8>("/lamp/omitted").unwrap();
    assert!(matches!(
        initial.read(&omitted),
        Err(tanuki_client::ReadError::NotObserved { .. })
    ));
    assert!(matches!(
        session
            .observe_batch([TopicPath::parse("/battery/absent").unwrap()])
            .await,
        Err(ClientError::NotSubscribed(_))
    ));
    let mut batch = session.batch();
    batch.publish(&hue, &120, ExpiryUpdate::Clear).unwrap();
    batch
        .publish(&brightness, &70, ExpiryUpdate::Clear)
        .unwrap();
    batch.commit().await.unwrap();
    let pair = receive(observed.recv()).await.unwrap().unwrap();
    assert_eq!(pair.nodes().len(), 2);
    assert_eq!(
        pair.read(&hue).unwrap().unwrap().current().unwrap().value,
        120
    );
    assert_eq!(
        pair.read(&brightness)
            .unwrap()
            .unwrap()
            .current()
            .unwrap()
            .value,
        70
    );
    hue.publish(&35, ExpiryUpdate::Clear).await.unwrap();
    let next = receive(observed.recv()).await.unwrap().unwrap();
    assert_eq!(
        next.read(&hue).unwrap().unwrap().current().unwrap().value,
        35
    );
    assert_eq!(
        pair.read(&hue).unwrap().unwrap().current().unwrap().value,
        120
    );
    assert!(initial.nodes().is_empty());
    hue.remove().await.unwrap();
    let removed = receive(observed.recv()).await.unwrap().unwrap();
    assert!(removed.read(&hue).unwrap().is_none());
    drop(observed);
    brightness.publish(&71, ExpiryUpdate::Clear).await.unwrap();
    session.close().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn a_full_raw_queue_reports_lag_once_without_stalling_another_listener() {
    use tanuki_client::{Session, SessionEnd, SessionOptions};
    let server = Server::start().await;
    let options = SessionOptions {
        raw_capacity: std::num::NonZeroUsize::new(1).unwrap(),
        ..Default::default()
    };
    let session = Session::connect_with_options(
        &server.url,
        ClientName::parse("two consumers").unwrap(),
        vec![Selector::parse("/battery/*").unwrap()],
        options,
    )
    .await
    .unwrap();
    let mut slow = session.listen_updates().await.unwrap();
    let mut fast = session.listen_updates().await.unwrap();
    for value in [80, 81, 82] {
        session.write(vec![publish(value)]).await.unwrap();
        receive(fast.recv()).await.unwrap().unwrap();
    }
    assert!(matches!(
        receive(slow.recv()).await,
        Err(SessionEnd::Lagged)
    ));
    assert!(receive(slow.recv()).await.unwrap().is_none());
    let fresh = session.listen_updates().await.unwrap();
    assert_eq!(fresh.initial_snapshot().nodes.len(), 1);
    session.write(vec![publish(83)]).await.unwrap();
    receive(fast.recv()).await.unwrap().unwrap();
    session.close().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn replacement_and_drop_end_listeners_but_preserve_weak_handle_lifetime() {
    use tanuki_client::{ClientError, ConnectionError, ExpiryUpdate, Session, SessionEnd};
    let server = Server::start().await;
    let first = Session::connect(&server.url, ClientName::parse("same name").unwrap(), vec![])
        .await
        .unwrap();
    let mut raw = first.listen_updates().await.unwrap();
    let second = Session::connect(&server.url, ClientName::parse("same name").unwrap(), vec![])
        .await
        .unwrap();
    assert!(matches!(
        second.hello_warnings(),
        [DiagnosticView::SessionReplaced { .. }]
    ));
    assert!(
        matches!(receive(raw.recv()).await, Err(SessionEnd::Connection(source)) if matches!(&*source, ConnectionError::Closed { code:4001, .. }))
    );
    assert!(receive(raw.recv()).await.unwrap().is_none());
    assert!(matches!(first.closed().await, SessionEnd::Connection(_)));
    drop(first);
    let handle = second.state::<u8>("/battery/laptop").unwrap();
    let mut raw = second.listen_updates().await.unwrap();
    drop(second);
    assert!(matches!(
        handle.publish(&80, ExpiryUpdate::Clear).await,
        Err(ClientError::SessionGone)
    ));
    assert!(receive(raw.recv()).await.unwrap().is_none());
    server.stop().await;
}

#[tokio::test]
async fn observation_keeps_null_missing_kind_errors_and_metadata_changes_distinct() {
    use tanuki_client::{ClaimRelease, ExpiryUpdate, ReadError, Session};
    let server = Server::start().await;
    let session = Session::connect(
        &server.url,
        ClientName::parse("null reader").unwrap(),
        vec![Selector::parse("/lamp/**").unwrap()],
    )
    .await
    .unwrap();
    let desired = session.desired::<Option<u8>>("/lamp/desired").unwrap();
    let mut observer = session.observe(&desired).await.unwrap();
    assert!(
        observer
            .recv()
            .await
            .unwrap()
            .unwrap()
            .read(&desired)
            .unwrap()
            .is_none()
    );
    desired.define().await.unwrap();
    let defined = receive(observer.recv()).await.unwrap().unwrap();
    assert!(defined.read(&desired).unwrap().unwrap().current().is_none());
    desired.submit(&None, ExpiryUpdate::Clear).await.unwrap();
    let null = receive(observer.recv()).await.unwrap().unwrap();
    assert_eq!(
        null.read(&desired)
            .unwrap()
            .unwrap()
            .current()
            .unwrap()
            .value,
        None
    );
    desired
        .claim(ClaimRelease::After(
            NonNegativeDuration::new(jiff::SignedDuration::from_secs(5)).unwrap(),
        ))
        .await
        .unwrap();
    let claimed = receive(observer.recv()).await.unwrap().unwrap();
    assert!(claimed.sequence() > null.sequence());
    let NodeView::Desired { claim, .. } = claimed.node(desired.path()).unwrap() else {
        panic!()
    };
    assert!(claim.is_some());
    desired.clear().await.unwrap();
    assert!(
        observer
            .recv()
            .await
            .unwrap()
            .unwrap()
            .read(&desired)
            .unwrap()
            .unwrap()
            .current()
            .is_none()
    );
    let wrong = session.state::<u8>("/lamp/desired").unwrap();
    wrong.publish(&70, ExpiryUpdate::Clear).await.unwrap();
    let changed = receive(observer.recv()).await.unwrap().unwrap();
    assert!(matches!(
        changed.read(&desired),
        Err(ReadError::WrongKind { .. })
    ));
    let wrong_payload = session.state::<String>("/lamp/desired").unwrap();
    wrong_payload
        .publish(&"bad".into(), ExpiryUpdate::Clear)
        .await
        .unwrap();
    let changed = receive(observer.recv()).await.unwrap().unwrap();
    assert!(matches!(
        changed.read(&wrong),
        Err(ReadError::Payload { .. })
    ));
    assert!(changed.node(wrong.path()).is_some());
    wrong.publish(&71, ExpiryUpdate::Clear).await.unwrap();
    assert_eq!(
        observer
            .recv()
            .await
            .unwrap()
            .unwrap()
            .read(&wrong)
            .unwrap()
            .unwrap()
            .current()
            .unwrap()
            .value,
        71
    );
    session.close().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn instant_occurrences_remain_ordered_batches_and_are_not_replayed_to_late_listeners() {
    use tanuki_client::{ExpiryUpdate, Session};
    let server = Server::start().await;
    let session = Session::connect(
        &server.url,
        ClientName::parse("instant probe").unwrap(),
        vec![Selector::parse("/lamp/**").unwrap()],
    )
    .await
    .unwrap();
    let event = session.event::<u8>("/lamp/event").unwrap();
    let command = session.command::<u8>("/lamp/command").unwrap();
    let mut raw = session.listen_updates().await.unwrap();
    command.define().await.unwrap();
    receive(raw.recv()).await.unwrap().unwrap();
    let mut batch = session.batch();
    batch
        .emit(&event, &1)
        .unwrap()
        .emit(&event, &2)
        .unwrap()
        .command(&command, &3)
        .unwrap()
        .command(&command, &4)
        .unwrap();
    batch.commit().await.unwrap();
    let update = receive(raw.recv()).await.unwrap().unwrap();
    let mut view = tanuki_client::SelectedView::from_snapshot(raw.initial_snapshot().clone());
    let applied = view.apply(update).unwrap();
    assert_eq!(applied.occurrences().len(), 4);
    let values: Vec<_> = applied
        .occurrences()
        .iter()
        .map(|change| match change {
            ChangeView::Event { event, .. } => event.value.as_inner(),
            ChangeView::Command { command, .. } => command.value.as_inner(),
            _ => panic!(),
        })
        .collect();
    assert_eq!(
        values,
        [
            &Value::Integer(1),
            &Value::Integer(2),
            &Value::Integer(3),
            &Value::Integer(4)
        ]
    );
    let late = session.listen_updates().await.unwrap();
    assert!(matches!(
        late.initial_snapshot().nodes.get("/lamp/event"),
        Some(NodeView::Event { .. })
    ));
    assert!(
        event
            .decode(late.initial_snapshot().nodes.get("/lamp/event").unwrap())
            .unwrap()
            .current()
            .is_none()
    );
    assert!(
        command
            .decode(late.initial_snapshot().nodes.get("/lamp/command").unwrap())
            .unwrap()
            .current()
            .is_none()
    );
    let state = session.state::<u8>("/lamp/state").unwrap();
    state.publish(&1, ExpiryUpdate::Clear).await.unwrap();
    receive(raw.recv()).await.unwrap().unwrap();
    state.remove().await.unwrap();
    assert!(matches!(
        &receive(raw.recv()).await.unwrap().unwrap().changes[..],
        [ChangeView::Removed {
            previous: NodeView::State { .. },
            ..
        }]
    ));
    session.close().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn sdk_battery_producer_and_dashboard_combine_with_stateless_phone_http() {
    use tanuki_client::{ExpiryUpdate, Session};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let server = Server::start().await;
    let producer = Session::connect(&server.url, ClientName::parse("laptop").unwrap(), vec![])
        .await
        .unwrap();
    let dashboard = Session::connect(
        &server.url,
        ClientName::parse("battery dashboard").unwrap(),
        vec![
            Selector::parse("/battery/*").unwrap(),
            Selector::parse("/battery/**").unwrap(),
        ],
    )
    .await
    .unwrap();
    let laptop = producer.state::<u8>("/battery/laptop").unwrap();
    let laptop_read = dashboard.state::<u8>("/battery/laptop").unwrap();
    let phone = dashboard.state::<u8>("/battery/phone").unwrap();
    let mut observed = dashboard
        .observe_batch([laptop_read.path().clone(), phone.path().clone()])
        .await
        .unwrap();
    assert!(
        receive(observed.recv())
            .await
            .unwrap()
            .unwrap()
            .nodes()
            .is_empty()
    );
    laptop.publish(&82, ExpiryUpdate::Clear).await.unwrap();
    let first = receive(observed.recv()).await.unwrap().unwrap();
    assert_eq!(first.nodes().len(), 1);
    let address = server
        .url
        .strip_prefix("ws://")
        .unwrap()
        .strip_suffix("/v1/ws")
        .unwrap();
    let body = r#"{"value":63,"expiry":{"mode":"clear"}}"#;
    let mut http = tokio::net::TcpStream::connect(address).await.unwrap();
    let request = format!(
        "POST /v1/state/battery/phone HTTP/1.1\r\nHost: {address}\r\nTanuki-Client: laptop\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    http.write_all(request.as_bytes()).await.unwrap();
    let mut reply = vec![];
    http.read_to_end(&mut reply).await.unwrap();
    assert!(String::from_utf8(reply).unwrap().contains("\"ok\":true"));
    let both = receive(observed.recv()).await.unwrap().unwrap();
    assert_eq!(both.nodes().len(), 2);
    assert_eq!(
        both.read(&laptop_read)
            .unwrap()
            .unwrap()
            .current()
            .unwrap()
            .value,
        82
    );
    assert_eq!(
        both.read(&phone).unwrap().unwrap().current().unwrap().value,
        63
    );
    assert_eq!(server.core.lock().unwrap().managed_session_count(), 2);
    laptop.publish(&83, ExpiryUpdate::Clear).await.unwrap();
    receive(observed.recv()).await.unwrap().unwrap();
    dashboard.close().await.unwrap();
    producer.close().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn both_socket_codecs_preserve_bytes_semantic_time_and_literal_tag_maps_in_requests_updates_and_snapshots()
 {
    use std::collections::BTreeMap;
    use tanuki_client::{Session, SessionOptions};
    let values = [
        Value::Float(FiniteF64::new(2.291712365432881e-9).unwrap()),
        Value::Float(FiniteF64::new(f64::MAX).unwrap()),
        Value::Bytes(vec![0, 255]),
        Value::Timestamp("2023-11-14T22:13:20.123456789Z".parse().unwrap()),
        Value::Duration("-PT5.125S".parse().unwrap()),
        Value::Map(BTreeMap::from([(
            "$bytes".into(),
            Value::String("literal".into()),
        )])),
        Value::Integer(i64::MIN),
        Value::Null,
    ];
    for codec in [Codec::Json, Codec::MessagePack] {
        let server = Server::start().await;
        let session = Session::connect_with_options(
            &server.url,
            ClientName::parse("codec probe").unwrap(),
            vec![Selector::parse("/battery/*").unwrap()],
            SessionOptions {
                codec,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let mut raw = session.listen_updates().await.unwrap();
        for value in &values {
            session
                .write(vec![WireOperation::PublishState {
                    topic: TopicPath::parse("/battery/laptop").unwrap(),
                    value: JsonValue::new(value.clone()),
                    expiry: Some(WireExpiry::Clear),
                }])
                .await
                .unwrap();
            let update = receive(raw.recv()).await.unwrap().unwrap();
            let ChangeView::Upsert {
                node: NodeView::State { current },
                ..
            } = &update.changes[0]
            else {
                panic!()
            };
            assert_eq!(current.value.as_inner(), value);
            let late = session.listen_updates().await.unwrap();
            let NodeView::State { current } = &late.initial_snapshot().nodes["/battery/laptop"]
            else {
                panic!()
            };
            assert_eq!(current.value.as_inner(), value);
            let viewer = Session::connect_with_options(
                &server.url,
                ClientName::parse("snapshot probe").unwrap(),
                vec![Selector::parse("/battery/*").unwrap()],
                SessionOptions {
                    codec,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            let NodeView::State { current } = &viewer.initial_snapshot().nodes["/battery/laptop"]
            else {
                panic!()
            };
            assert_eq!(current.value.as_inner(), value);
            viewer.close().await.unwrap();
        }
        session.close().await.unwrap();
        server.stop().await;
    }
}

async fn receive<T, E: std::fmt::Debug>(
    future: impl std::future::Future<Output = Result<Option<T>, E>>,
) -> Result<Option<T>, E> {
    tokio::time::timeout(std::time::Duration::from_secs(3), future)
        .await
        .expect("SDK receive stalled")
}

#[tokio::test]
async fn sdk_observers_receive_timer_removal_and_desired_expiry_without_losing_claim() {
    use tanuki_client::{ClaimRelease, ExpiryUpdate, Session};
    let server = Server::start().await;
    let session = Session::connect(
        &server.url,
        ClientName::parse("expiry probe").unwrap(),
        vec![Selector::parse("/lamp/**").unwrap()],
    )
    .await
    .unwrap();
    let state = session.state::<u8>("/lamp/brightness").unwrap();
    let desired = session.desired::<u8>("/lamp/desired").unwrap();
    let mut observer = session
        .observe_batch([state.path().clone(), desired.path().clone()])
        .await
        .unwrap();
    receive(observer.recv()).await.unwrap().unwrap();
    let duration = NonNegativeDuration::new(jiff::SignedDuration::from_secs(5)).unwrap();
    let mut setup = session.batch();
    setup
        .define(&desired)
        .unwrap()
        .claim(&desired, ClaimRelease::Immediate)
        .unwrap()
        .submit(&desired, &70, ExpiryUpdate::Set(duration))
        .unwrap()
        .publish(&state, &70, ExpiryUpdate::Set(duration))
        .unwrap();
    setup.commit().await.unwrap();
    let before = receive(observer.recv()).await.unwrap().unwrap();
    assert_eq!(before.nodes().len(), 2);
    // Invoke the production deadline mutation with controlled wall time; transport remains real.
    server
        .core
        .lock()
        .unwrap()
        .process_deadlines(
            Timestamp::new(jiff::Timestamp::from_second(1700000005).unwrap()),
            &[],
        )
        .unwrap();
    let expired = receive(observer.recv()).await.unwrap().unwrap();
    assert!(expired.read(&state).unwrap().is_none());
    let NodeView::Desired { claim, current, .. } = expired.node(desired.path()).unwrap() else {
        panic!()
    };
    assert!(claim.is_some());
    assert!(current.is_none());
    assert_eq!(
        before
            .read(&state)
            .unwrap()
            .unwrap()
            .current()
            .unwrap()
            .value,
        70
    );
    session.close().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn registration_racing_with_commits_loses_no_retained_update_and_observer_drop_is_independent()
 {
    use tanuki_client::{ExpiryUpdate, Session};
    let server = Server::start().await;
    let producer = Session::connect(
        &server.url,
        ClientName::parse("race producer").unwrap(),
        vec![],
    )
    .await
    .unwrap();
    let dashboard = Session::connect(
        &server.url,
        ClientName::parse("race dashboard").unwrap(),
        vec![Selector::parse("/battery/*").unwrap()],
    )
    .await
    .unwrap();
    let topic = producer.state::<u8>("/battery/laptop").unwrap();
    let reading = dashboard.state::<u8>("/battery/laptop").unwrap();
    for value in 1..=20u8 {
        let (raw, receipt) = tokio::join!(
            dashboard.listen_updates(),
            topic.publish(&value, ExpiryUpdate::Clear)
        );
        receipt.unwrap();
        let mut raw = raw.unwrap();
        let mut view = tanuki_client::SelectedView::from_snapshot(raw.initial_snapshot().clone());
        let current = view
            .nodes()
            .get("/battery/laptop")
            .and_then(|node| reading.decode(node).ok())
            .and_then(|node| node.current().map(|c| c.value));
        if current != Some(value) {
            view.apply(receive(raw.recv()).await.unwrap().unwrap())
                .unwrap();
        }
        assert_eq!(
            reading
                .decode(&view.nodes()["/battery/laptop"])
                .unwrap()
                .current()
                .unwrap()
                .value,
            value
        );
    }
    let observer = dashboard.observe(&reading).await.unwrap();
    let mut another = dashboard.observe(&reading).await.unwrap();
    receive(another.recv()).await.unwrap().unwrap();
    drop(observer);
    topic.publish(&21, ExpiryUpdate::Clear).await.unwrap();
    assert_eq!(
        receive(another.recv())
            .await
            .unwrap()
            .unwrap()
            .read(&reading)
            .unwrap()
            .unwrap()
            .current()
            .unwrap()
            .value,
        21
    );
    dashboard.close().await.unwrap();
    producer.close().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn direct_empty_producer_selection_exposes_replacement_close_reason_once() {
    use tanuki_client::ConnectionError;
    let server = Server::start().await;
    let mut first = Connection::open(&server.url).await.unwrap();
    first.send(&hello("direct duplicate", &[])).await.unwrap();
    assert!(
        matches!(receive(first.recv()).await.unwrap(),Some(ServerMessage::Snapshot { nodes,.. }) if nodes.is_empty())
    );
    let mut replacement = Connection::open(&server.url).await.unwrap();
    replacement
        .send(&hello("direct duplicate", &[]))
        .await
        .unwrap();
    assert!(
        matches!(receive(replacement.recv()).await.unwrap(),Some(ServerMessage::Snapshot { warnings,.. }) if matches!(&warnings[..],[DiagnosticView::SessionReplaced { .. }]))
    );
    assert!(
        matches!(receive(first.recv()).await,Err(ConnectionError::Closed { code:4001,reason }) if reason == "session_replaced")
    );
    assert!(receive(first.recv()).await.unwrap().is_none());
    replacement.close().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn standard_stream_observation_is_native_send_and_ends_after_joined_close() {
    use futures_util::{StreamExt, pin_mut};
    use tanuki_client::{ExpiryUpdate, Session};
    fn assert_send<T: Send>(_: &T) {}
    let server = Server::start().await;
    let session = Session::connect(
        &server.url,
        ClientName::parse("stream dashboard").unwrap(),
        vec![Selector::parse("/battery/*").unwrap()],
    )
    .await
    .unwrap();
    let topic = session.state::<u8>("/battery/laptop").unwrap();
    assert_send(&topic);
    let pending = session.observe(&topic);
    assert_send(&pending);
    let observer = pending.await.unwrap();
    let stream = observer.into_stream();
    assert_send(&stream);
    pin_mut!(stream);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .nodes()
            .is_empty()
    );
    topic.publish(&82, ExpiryUpdate::Clear).await.unwrap();
    let snapshot = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        snapshot
            .read(&topic)
            .unwrap()
            .unwrap()
            .current()
            .unwrap()
            .value,
        82
    );
    session.close().await.unwrap();
    assert!(stream.next().await.is_none());
    server.stop().await;
}

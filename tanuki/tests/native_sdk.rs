//! The Rust SDK against the real server over loopback WebSockets.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tanuki::{core::Core, domain::Timestamp, server::serve_with_core, transport::SharedCore};
use tanuki_client::protocol::*;
use tanuki_client::{
    ClaimRelease, ClientError, Closed, Codec, CommandTopic, Connection, DesiredTopic, Event,
    EventTopic, Expiry, Observer, Reconnect, Request, Resync, Session, Snapshot, StateTopic,
    Status,
};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle, time::timeout};

struct Server {
    address: SocketAddr,
    url: String,
    core: SharedCore,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}

impl Server {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        Self::serve(listener, Arc::new(Mutex::new(Core::new())))
    }

    /// Restarts on the same address with the same state, like a server
    /// process restart without losing retained data.
    async fn restart(address: SocketAddr, core: SharedCore) -> Self {
        Self::serve(TcpListener::bind(address).await.unwrap(), core)
    }

    fn serve(listener: TcpListener, core: SharedCore) -> Self {
        let address = listener.local_addr().unwrap();
        let (shutdown, stopped) = oneshot::channel();
        // A real clock: controllers tell submissions apart by provenance.
        let task = tokio::spawn(serve_with_core(
            listener,
            core.clone(),
            Arc::new(|| Timestamp::new(jiff::Timestamp::now())),
            async {
                let _ = stopped.await;
            },
        ));
        Self {
            address,
            url: format!("ws://{address}/v1/ws"),
            core,
            shutdown,
            task,
        }
    }

    async fn stop(self) -> (SocketAddr, SharedCore) {
        let _ = self.shutdown.send(());
        self.task.await.unwrap().unwrap();
        (self.address, self.core)
    }
}

async fn soon<T>(future: impl std::future::Future<Output = T>) -> T {
    timeout(Duration::from_secs(5), future)
        .await
        .expect("timed out")
}

async fn next(observer: &mut Observer) -> Snapshot {
    soon(observer.recv()).await.expect("observer ended")
}

async fn wait_for_status(session: &Session, wanted: impl Fn(&Status) -> bool) -> Status {
    let mut status = session.status();
    soon(async {
        loop {
            let current = status.current();
            if wanted(&current) {
                return current;
            }
            status.changed().await.expect("session ended");
        }
    })
    .await
}

fn hello(name: &str, selectors: &[&str]) -> ClientMessage {
    ClientMessage::Hello {
        request_id: RequestId::new("hello".into()),
        client: ClientName::parse(name).unwrap(),
        selectors: selectors
            .iter()
            .map(|s| Selector::parse(s).unwrap())
            .collect(),
        session: SessionMode::Managed,
    }
}

fn publish(value: i64) -> WireOperation {
    WireOperation::PublishState {
        topic: TopicPath::parse("/battery/laptop").unwrap(),
        value: WireValue::new(Value::Integer(value)),
        expiry: Some(WireExpiry::Clear),
    }
}

#[tokio::test]
async fn direct_connection_exchanges_hello_update_reply_and_errors_in_both_codecs() {
    let server = Server::start().await;
    for codec in [Codec::Json, Codec::MessagePack] {
        let mut connection = Connection::open_with_codec(&server.url, codec)
            .await
            .unwrap();
        connection
            .send(&hello(&format!("direct {codec:?}"), &["/battery/*"]))
            .await
            .unwrap();
        assert!(matches!(
            soon(connection.recv()).await.unwrap(),
            Some(ServerMessage::Snapshot { request_id, .. }) if request_id.as_str() == "hello"
        ));
        // Mixed codecs: each request frame chooses its own reply codec.
        let other = if codec == Codec::Json {
            Codec::MessagePack
        } else {
            Codec::Json
        };
        connection
            .send_with_codec(
                &ClientMessage::Write {
                    request_id: RequestId::new("write".into()),
                    operations: vec![publish(82)],
                },
                other,
            )
            .await
            .unwrap();
        assert!(matches!(
            soon(connection.recv()).await.unwrap(),
            Some(ServerMessage::Update { changes, .. }) if changes.len() == 1
        ));
        assert!(matches!(
            soon(connection.recv()).await.unwrap(),
            Some(ServerMessage::Reply { request_id, .. }) if request_id.as_str() == "write"
        ));
        connection
            .send(&ClientMessage::Write {
                request_id: RequestId::new("invalid".into()),
                operations: vec![],
            })
            .await
            .unwrap();
        assert!(matches!(
            soon(connection.recv()).await.unwrap(),
            Some(ServerMessage::Error { request_id: Some(id), error })
                if id.as_str() == "invalid" && error.code == ErrorCode::EmptyBatch
        ));
        connection.close().await.unwrap();
    }
    server.stop().await;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Battery {
    level: u8,
    charging: bool,
}

#[tokio::test]
async fn observers_see_wildcards_typed_values_and_their_own_writes() {
    let server = Server::start().await;
    let laptop = Session::connect(&server.url, "laptop").await.unwrap();
    let phone = Session::builder(&server.url, "phone")
        .stateless()
        .connect()
        .await
        .unwrap();
    let laptop_battery = StateTopic::<Battery>::new("/battery/laptop").unwrap();
    let phone_battery = StateTopic::<Battery>::new("/battery/phone").unwrap();

    let mut batteries = laptop.observe("/battery/*").unwrap();
    assert!(next(&mut batteries).await.is_empty());

    let state = Battery {
        level: 82,
        charging: false,
    };
    let receipt = laptop.publish(&laptop_battery, &state).await.unwrap();
    // Read-your-writes: the observer holds the commit before its receipt resolves.
    let latest = batteries.latest().unwrap();
    assert!(latest.sequence() >= receipt.sequence);
    assert_eq!(latest.get(&laptop_battery).unwrap(), Some(state.clone()));
    assert_eq!(next(&mut batteries).await.sequence(), latest.sequence());

    phone
        .publish(
            &phone_battery,
            &Battery {
                level: 40,
                charging: true,
            },
        )
        .await
        .unwrap();
    let snapshot = next(&mut batteries).await;
    let levels: Vec<(String, u8)> = snapshot
        .values::<Battery>()
        .map(|(path, battery)| (path.to_string(), battery.unwrap().level))
        .collect();
    assert_eq!(
        levels,
        [
            ("/battery/laptop".into(), 82),
            ("/battery/phone".into(), 40)
        ]
    );
    let Some(tanuki_client::Node::State(current)) = snapshot.read(&phone_battery).unwrap() else {
        panic!("the phone battery is state");
    };
    assert_eq!(current.last_write.client, "phone");
    assert_eq!(current.last_write.session, None);

    let lamp = StateTopic::<u8>::new("/lamp/brightness").unwrap();
    assert!(matches!(
        snapshot.get(&lamp),
        Err(tanuki_client::ReadError::NotObserved { .. })
    ));
    let wrong = DesiredTopic::<Battery>::new("/battery/laptop").unwrap();
    assert!(matches!(
        snapshot.get(&wrong),
        Err(tanuki_client::ReadError::WrongKind { .. })
    ));

    laptop.close().await;
    phone.close().await;
    server.stop().await;
}

#[tokio::test]
async fn batches_commit_atomically_and_report_encoding_and_rejection_errors() {
    let server = Server::start().await;
    let session = Session::connect(&server.url, "lamp").await.unwrap();
    let hue = StateTopic::<u16>::new("/lamp/hue").unwrap();
    let brightness = StateTopic::<u8>::new("/lamp/brightness").unwrap();
    let mut lamp = session.observe((&hue, &brightness)).unwrap();
    let empty = next(&mut lamp).await;

    session
        .batch()
        .publish(&hue, &120)
        .publish(&brightness, &70)
        .commit()
        .await
        .unwrap();
    let both = next(&mut lamp).await;
    assert_eq!(both.get(&hue).unwrap(), Some(120));
    assert_eq!(both.get(&brightness).unwrap(), Some(70));

    // A payload Serde cannot express fails at commit; nothing is sent.
    let odd = StateTopic::<BTreeMap<(u8, u8), u8>>::new("/lamp/odd").unwrap();
    assert!(matches!(
        session
            .batch()
            .publish(&hue, &1)
            .publish(&odd, &BTreeMap::from([((1, 2), 3)]))
            .commit()
            .await,
        Err(ClientError::Payload(PayloadError::MapKey))
    ));

    // A server rejection leaves no partial effect.
    let undefined = DesiredTopic::<u8>::new("/lamp/undefined").unwrap();
    assert!(matches!(
        session
            .batch()
            .publish(&hue, &2)
            .submit(&undefined, &1)
            .commit()
            .await,
        Err(ClientError::Rejected(error)) if error.code == ErrorCode::InputNotDefined
    ));
    assert!(matches!(
        session.batch().commit().await,
        Err(ClientError::EmptyBatch)
    ));
    let unchanged = lamp.latest().unwrap();
    assert_eq!(unchanged.sequence(), both.sequence());
    assert_eq!(unchanged.get(&hue).unwrap(), Some(120));
    assert!(empty.sequence() < both.sequence());

    session
        .publish_with(&brightness, &71, Expiry::After(Duration::from_secs(60)))
        .await
        .unwrap();
    let Some(tanuki_client::Node::State(current)) =
        lamp.latest().unwrap().read(&brightness).unwrap()
    else {
        panic!("brightness is state");
    };
    let lease = current
        .expires_at
        .unwrap()
        .duration_since(current.last_write.at);
    assert_eq!(lease, jiff::SignedDuration::from_secs(60));

    session.close().await;
    server.stop().await;
}

#[tokio::test]
async fn a_controller_receives_each_request_until_its_claim_is_taken() {
    let server = Server::start().await;
    let controller = Session::connect(&server.url, "lamp controller")
        .await
        .unwrap();
    let remote = Session::builder(&server.url, "remote")
        .stateless()
        .connect()
        .await
        .unwrap();
    let desired = DesiredTopic::<u8>::new("/lamp/desired").unwrap();

    // Intent submitted before any controller exists waits for one.
    remote
        .batch()
        .define(&desired)
        .submit(&desired, &30)
        .commit()
        .await
        .unwrap();
    let mut control = controller
        .control(&desired, ClaimRelease::Immediate)
        .await
        .unwrap();
    let Request::Submitted(pending) = soon(control.recv()).await.unwrap().unwrap() else {
        panic!("the pending request comes first");
    };
    assert_eq!(pending.value, 30);
    assert_eq!(pending.last_write.client, "remote");
    // Nothing new was submitted: the controller stays quiet.
    assert!(
        timeout(Duration::from_millis(100), control.recv())
            .await
            .is_err(),
        "a request is reported once"
    );

    remote.submit(&desired, &70).await.unwrap();
    assert!(matches!(
        soon(control.recv()).await.unwrap().unwrap(),
        Request::Submitted(request) if request.value == 70
    ));
    // Repeating the same value is a new request.
    remote.submit(&desired, &70).await.unwrap();
    assert!(matches!(
        soon(control.recv()).await.unwrap().unwrap(),
        Request::Submitted(request) if request.value == 70
    ));
    remote.clear(&desired).await.unwrap();
    assert_eq!(
        soon(control.recv()).await.unwrap().unwrap(),
        Request::Cleared
    );

    let thief = Session::connect(&server.url, "other controller")
        .await
        .unwrap();
    let _stolen = thief
        .control(&desired, ClaimRelease::Immediate)
        .await
        .unwrap();
    assert!(matches!(
        soon(control.recv()).await.unwrap(),
        Err(tanuki_client::ControlError::ClaimLost { by: Some(by), .. }) if by == "other controller"
    ));
    assert!(soon(control.recv()).await.is_none());

    // Stateless sessions cannot claim.
    assert!(matches!(
        remote.control(&desired, ClaimRelease::Immediate).await,
        Err(ClientError::Rejected(error)) if error.code == ErrorCode::SessionRequired
    ));

    for session in [controller, remote, thief] {
        session.close().await;
    }
    server.stop().await;
}

#[tokio::test]
async fn events_and_commands_arrive_typed_and_in_order() {
    let server = Server::start().await;
    let device = Session::connect(&server.url, "doorbell").await.unwrap();
    let remote = Session::builder(&server.url, "remote")
        .stateless()
        .connect()
        .await
        .unwrap();
    let rang = EventTopic::<String>::new("/doorbell/rang").unwrap();
    let chime = CommandTopic::<u8>::new("/doorbell/chime").unwrap();

    let mut rings = remote.occurrences(&rang).unwrap();
    let mut chimes = device
        .control(&chime, ClaimRelease::Immediate)
        .await
        .unwrap();
    // Occurrences start after registration takes effect: observe a write first.
    let marker = StateTopic::<u8>::new("/doorbell/marker").unwrap();
    let mut barrier = remote.observe((&rang, &marker)).unwrap();
    next(&mut barrier).await;

    device
        .batch()
        .emit(&rang, &"front".to_owned())
        .emit(&rang, &"back".to_owned())
        .commit()
        .await
        .unwrap();
    let first = soon(rings.recv()).await.unwrap().unwrap();
    let second = soon(rings.recv()).await.unwrap().unwrap();
    assert_eq!(
        (first.value.as_str(), second.value.as_str()),
        ("front", "back")
    );
    assert_eq!(first.sequence, second.sequence);
    assert_eq!(first.provenance.client, "doorbell");

    remote.submit(&chime, &3).await.unwrap();
    let command = soon(chimes.recv()).await.unwrap().unwrap();
    assert_eq!(command.value, 3);
    assert_eq!(command.provenance.client, "remote");

    device.close().await;
    remote.close().await;
    server.stop().await;
}

#[tokio::test]
async fn a_lagging_listener_is_resynchronised_with_the_current_state() {
    let server = Server::start().await;
    let reader = Session::builder(&server.url, "slow reader")
        .listener_capacity(2)
        .connect()
        .await
        .unwrap();
    let writer = Session::connect(&server.url, "fast writer").await.unwrap();
    let counter = StateTopic::<u32>::new("/burst/counter").unwrap();
    let mut listener = reader.listen(&counter).unwrap();
    assert!(matches!(
        soon(listener.recv()).await,
        Some(Event::Snapshot {
            reason: Resync::Started,
            ..
        })
    ));
    let mut done = reader.observe(&counter).unwrap();
    for value in 1..=6 {
        writer.publish(&counter, &value).await.unwrap();
    }
    while next(&mut done).await.get(&counter).unwrap() != Some(6) {}

    let mut lagged = false;
    let mut last = None;
    while let Ok(Some(event)) = timeout(Duration::from_millis(100), listener.recv()).await {
        match event {
            Event::Snapshot { snapshot, reason } => {
                lagged |= reason == Resync::Lagged;
                last = snapshot.get(&counter).unwrap();
            }
            Event::Update(update) => {
                let [ChangeView::Upsert { node, .. }] = update.changes.as_slice() else {
                    panic!("one upsert per write");
                };
                last = Some(node.current().unwrap().decode().unwrap());
            }
        }
    }
    assert!(lagged, "six writes overflow a two-event queue");
    assert_eq!(last, Some(6));
    reader.close().await;
    writer.close().await;
    server.stop().await;
}

#[tokio::test]
async fn sessions_reconnect_resume_observers_and_reclaim_inputs() {
    let server = Server::start().await;
    let backoff = Reconnect::Backoff {
        initial: Duration::from_millis(20),
        max: Duration::from_millis(100),
    };
    let controller = Session::builder(&server.url, "lamp controller")
        .reconnect(backoff)
        .connect()
        .await
        .unwrap();
    let desired = DesiredTopic::<u8>::new("/lamp/desired").unwrap();
    let level = StateTopic::<u8>::new("/lamp/level").unwrap();
    let mut control = controller
        .control(&desired, ClaimRelease::Immediate)
        .await
        .unwrap();
    let mut lamp = controller.observe("/lamp/*").unwrap();
    next(&mut lamp).await;
    controller.publish(&level, &10).await.unwrap();

    let url = server.url.clone();
    let (address, core) = server.stop().await;
    wait_for_status(&controller, |status| {
        matches!(status, Status::Reconnecting { .. })
    })
    .await;
    assert!(matches!(
        controller.publish(&level, &11).await,
        Err(ClientError::Disconnected)
    ));

    let server = Server::restart(address, core).await;
    wait_for_status(&controller, |status| matches!(status, Status::Connected)).await;
    // The observer resumes from a fresh baseline with retained data intact.
    let resumed = next(&mut lamp).await;
    assert_eq!(resumed.get(&level).unwrap(), Some(10));

    // The input was re-claimed: a new request reaches the same controller.
    let remote = Session::builder(&url, "remote")
        .stateless()
        .connect()
        .await
        .unwrap();
    remote.submit(&desired, &55).await.unwrap();
    assert!(matches!(
        soon(control.recv()).await.unwrap().unwrap(),
        Request::Submitted(request) if request.value == 55
    ));
    controller.close().await;
    remote.close().await;
    server.stop().await;
}

#[tokio::test]
async fn start_keeps_trying_until_the_server_appears() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let session = Session::builder(&format!("ws://{address}/v1/ws"), "early dashboard")
        .stateless()
        .reconnect(Reconnect::Backoff {
            initial: Duration::from_millis(20),
            max: Duration::from_millis(50),
        })
        .start()
        .unwrap();
    let mut batteries = session.observe("/battery/*").unwrap();
    wait_for_status(&session, |status| {
        matches!(status, Status::Reconnecting { .. })
    })
    .await;

    let server = Server::restart(address, Arc::new(Mutex::new(Core::new()))).await;
    assert!(next(&mut batteries).await.is_empty());
    assert!(matches!(session.status().current(), Status::Connected));
    session.close().await;
    server.stop().await;
}

#[tokio::test]
async fn a_replaced_managed_session_ends_while_the_new_one_continues() {
    let server = Server::start().await;
    let old = Session::connect(&server.url, "laptop").await.unwrap();
    let new = Session::connect(&server.url, "laptop").await.unwrap();
    assert!(matches!(soon(old.closed()).await, Closed::Replaced));
    let level = StateTopic::<u8>::new("/battery/laptop").unwrap();
    new.publish(&level, &1).await.unwrap();
    new.close().await;
    server.stop().await;
}

#[tokio::test]
async fn closing_ends_observers_and_rejects_later_calls() {
    let server = Server::start().await;
    let session = Session::connect(&server.url, "dashboard").await.unwrap();
    let mut observer = session.observe("/**").unwrap();
    next(&mut observer).await;
    let clone = session.clone();
    clone.close().await;
    assert!(soon(observer.recv()).await.is_none());
    let level = StateTopic::<u8>::new("/battery/laptop").unwrap();
    assert!(matches!(
        session.publish(&level, &1).await,
        Err(ClientError::Closed(Closed::ByClient))
    ));
    assert!(matches!(
        session.observe("/**"),
        Err(ClientError::Closed(Closed::ByClient))
    ));

    // Dropping the last clone also ends the session and its observers.
    let dropped = Session::connect(&server.url, "dropped").await.unwrap();
    let mut orphan = dropped.observe("/**").unwrap();
    next(&mut orphan).await;
    drop(dropped);
    assert!(soon(orphan.recv()).await.is_none());
    server.stop().await;
}

#[tokio::test]
async fn both_codecs_preserve_bytes_semantic_time_and_literal_tag_maps() {
    let values = [
        Value::Float(FiniteF64::new(2.291712365432881e-9).unwrap()),
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
    let server = Server::start().await;
    let probe = StateTopic::<Value>::new("/codec/probe").unwrap();
    for codec in [Codec::Json, Codec::MessagePack] {
        let writer = Session::builder(&server.url, "codec writer")
            .codec(codec)
            .connect()
            .await
            .unwrap();
        let reader = Session::builder(&server.url, "codec reader")
            .codec(codec)
            .stateless()
            .connect()
            .await
            .unwrap();
        let mut observed = reader.observe(&probe).unwrap();
        next(&mut observed).await;
        for value in &values {
            writer.publish(&probe, value).await.unwrap();
            assert_eq!(
                next(&mut observed).await.get(&probe).unwrap().as_ref(),
                Some(value)
            );
            // A fresh baseline (snapshot path) carries the same value.
            let mut late = reader.observe(&probe).unwrap();
            assert_eq!(
                next(&mut late).await.get(&probe).unwrap().as_ref(),
                Some(value)
            );
        }
        writer.close().await;
        reader.close().await;
    }
    server.stop().await;
}

#[tokio::test]
async fn a_reconnecting_controller_does_not_take_back_an_input_claimed_meanwhile() {
    let server = Server::start().await;
    let slow = Session::builder(&server.url, "first controller")
        .reconnect(Reconnect::Backoff {
            initial: Duration::from_millis(400),
            max: Duration::from_millis(400),
        })
        .connect()
        .await
        .unwrap();
    let desired = DesiredTopic::<u8>::new("/lamp/desired").unwrap();
    let mut first = slow
        .control(&desired, ClaimRelease::Immediate)
        .await
        .unwrap();

    // While the first controller waits to reconnect, another takes over.
    let (address, core) = server.stop().await;
    let server = Server::restart(address, core).await;
    let other = Session::connect(&server.url, "second controller")
        .await
        .unwrap();
    let mut second = other
        .control(&desired, ClaimRelease::Immediate)
        .await
        .unwrap();

    wait_for_status(&slow, |status| matches!(status, Status::Connected)).await;
    assert!(matches!(
        soon(first.recv()).await.unwrap(),
        Err(tanuki_client::ControlError::ClaimRejected { error, .. })
            if error.code == ErrorCode::ClaimAuthorityRequired
    ));
    // The second controller keeps the input.
    let remote = Session::builder(&server.url, "remote")
        .stateless()
        .connect()
        .await
        .unwrap();
    remote.submit(&desired, &5).await.unwrap();
    assert!(matches!(
        soon(second.recv()).await.unwrap().unwrap(),
        Request::Submitted(request) if request.value == 5
    ));
    for session in [slow, other, remote] {
        session.close().await;
    }
    server.stop().await;
}

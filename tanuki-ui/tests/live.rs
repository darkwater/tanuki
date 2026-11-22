use std::{
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};
use tanuki::{
    core::{Core, SchemaInstallMode},
    domain::Timestamp,
    server::serve_with_core,
};
use tanuki_client::Connection;
use tanuki_protocol::*;
use tanuki_ui::{
    app::{App, AppError, ConnectionConfig},
    model::ConnectionStatus,
};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

struct Server {
    url: String,
    core: Arc<Mutex<Core>>,
    now: Arc<AtomicI64>,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}
impl Server {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/v1/ws", listener.local_addr().unwrap());
        let core = Arc::new(Mutex::new(Core::new()));
        let now = Arc::new(AtomicI64::new(1_700_000_000));
        let clock = now.clone();
        let (shutdown, stopped) = oneshot::channel();
        let task = tokio::spawn(serve_with_core(
            listener,
            core.clone(),
            Arc::new(move || at(clock.load(Ordering::SeqCst))),
            async {
                let _ = stopped.await;
            },
        ));
        Self {
            url,
            core,
            now,
            shutdown,
            task,
        }
    }
    fn app(&self, name: &str) -> App {
        App::new(ConnectionConfig::new(
            self.url.clone(),
            ClientName::parse(name).unwrap(),
        ))
    }
    async fn producer(&self) -> Writer {
        Writer::connect(&self.url, "producer", SessionMode::Stateless, &[]).await
    }
    async fn stop(self) {
        let _ = self.shutdown.send(());
        receive(self.task).await.unwrap().unwrap();
    }
}
fn at(second: i64) -> Timestamp {
    Timestamp::new(jiff::Timestamp::from_second(second).unwrap())
}
fn path(p: &str) -> TopicPath {
    TopicPath::parse(p).unwrap()
}

/// A minimal protocol-level writer for driving the inspector.
struct Writer {
    connection: Connection,
    next: u64,
}
impl Writer {
    async fn connect(url: &str, name: &str, session: SessionMode, selectors: &[&str]) -> Self {
        let mut connection = Connection::open(url).await.unwrap();
        connection
            .send(&ClientMessage::Hello {
                request_id: RequestId::new("hello".into()),
                client: ClientName::parse(name).unwrap(),
                selectors: selectors
                    .iter()
                    .map(|s| Selector::parse(s).unwrap())
                    .collect(),
                session,
            })
            .await
            .unwrap();
        assert!(matches!(
            receive(connection.recv()).await.unwrap(),
            Some(ServerMessage::Snapshot { .. })
        ));
        Self {
            connection,
            next: 0,
        }
    }
    /// Commits one batch and returns its sequence, skipping selected updates.
    async fn write(&mut self, operations: Vec<WireOperation>) -> u64 {
        self.next += 1;
        let id = format!("w{}", self.next);
        self.connection
            .send(&ClientMessage::Write {
                request_id: RequestId::new(id.clone()),
                operations,
            })
            .await
            .unwrap();
        loop {
            match receive(self.connection.recv()).await.unwrap() {
                Some(ServerMessage::Reply { request_id, result }) if request_id.as_str() == id => {
                    return result.sequence;
                }
                Some(ServerMessage::Error { error, .. }) => panic!("write rejected: {error:?}"),
                Some(_) => {}
                None => panic!("connection ended"),
            }
        }
    }
    async fn publish(&mut self, topic: &str, value: i64) -> u64 {
        self.write(vec![publish(topic, value, None)]).await
    }
    async fn close(self) {
        let _ = self.connection.close().await;
    }
}
fn publish(topic: &str, value: i64, expiry: Option<WireExpiry>) -> WireOperation {
    WireOperation::PublishState {
        topic: path(topic),
        value: Value::Integer(value),
        expiry,
    }
}
async fn receive<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("live inspector operation timed out")
}
async fn through(app: &mut App, sequence: u64) {
    receive(async {
        while app.model().sequence() < sequence {
            app.next_update().await.unwrap();
        }
    })
    .await;
}
fn value(app: &App, p: &str) -> i64 {
    let Some(NodeView::State { current }) = app.model().node(&path(p)) else {
        panic!("missing state {p}")
    };
    let Value::Integer(n) = &current.value else {
        panic!("not an integer")
    };
    *n
}

#[tokio::test]
async fn empty_populated_and_future_wildcard_topics_follow_the_live_listener() {
    let server = Server::start().await;
    let mut app = server.app("inspector");
    receive(app.reconnect()).await.unwrap();
    assert_eq!(app.model().status(), &ConnectionStatus::Live);
    assert_eq!(app.model().empty_message(), Some("No topics"));
    let mut producer = server.producer().await;
    let sequence = producer.publish("/later/deep/value", 42).await;
    through(&mut app, sequence).await;
    assert_eq!(value(&app, "/later/deep/value"), 42);
    let mut populated = server.app("second inspector");
    receive(populated.reconnect()).await.unwrap();
    assert_eq!(value(&populated, "/later/deep/value"), 42);
    app.close().await.unwrap();
    populated.close().await.unwrap();
    producer.close().await;
    server.stop().await;
}

#[tokio::test]
async fn whole_batches_metadata_and_scheduled_expiry_reach_the_model() {
    let server = Server::start().await;
    let mut producer = server.producer().await;
    let mut app = server.app("inspector");
    receive(app.reconnect()).await.unwrap();
    let sequence = producer
        .write(vec![
            publish("/lamp/hue", 120, None),
            publish("/lamp/brightness", 70, None),
        ])
        .await;
    receive(app.next_update()).await.unwrap();
    assert_eq!(app.model().sequence(), sequence);
    assert_eq!(
        (value(&app, "/lamp/hue"), value(&app, "/lamp/brightness")),
        (120, 70)
    );
    let before = app.model().node(&path("/lamp/hue")).unwrap().clone();
    server.now.fetch_add(1, Ordering::SeqCst);
    let sequence = producer
        .write(vec![publish(
            "/lamp/hue",
            120,
            Some(WireExpiry::Set {
                duration: "PT1H".into(),
            }),
        )])
        .await;
    through(&mut app, sequence).await;
    assert_ne!(app.model().node(&path("/lamp/hue")), Some(&before));
    assert_eq!(value(&app, "/lamp/hue"), 120);
    server.now.fetch_add(3600, Ordering::SeqCst);
    // A real transport mutation wakes the production scheduler after advancing
    // the injected wall clock. No timer sleeps or mock expiry behavior.
    producer.publish("/clock-tick", 1).await;
    receive(async {
        while app.model().node(&path("/lamp/hue")).is_some() {
            app.next_update().await.unwrap();
        }
    })
    .await;
    assert_eq!(value(&app, "/lamp/brightness"), 70);
    app.close().await.unwrap();
    producer.close().await;
    server.stop().await;
}

#[tokio::test]
async fn broad_subscription_includes_exposed_system_diagnostics() {
    use tanuki::schema::{Enforcement, NullPolicy, Schema, SchemaName, SchemaRule, ValueValidator};
    let server = Server::start().await;
    let now = at(server.now.load(Ordering::SeqCst));
    server
        .core
        .lock()
        .unwrap()
        .install_schema(
            Schema::new(
                SchemaName::parse("freshness").unwrap(),
                vec![
                    SchemaRule::new(
                        Selector::parse("/tracked/**").unwrap(),
                        Enforcement::Warn,
                        ValueValidator::any(NullPolicy::Allow),
                    )
                    .unwrap()
                    .with_expected_update_interval(
                        NonNegativeDuration::new(jiff::SignedDuration::from_secs(3600)).unwrap(),
                    )
                    .unwrap(),
                ],
            )
            .unwrap(),
            SchemaInstallMode::RejectInvalid,
            now,
        )
        .unwrap();
    let mut producer = server.producer().await;
    let mut explicit = Writer::connect(
        &server.url,
        "diagnostics listener",
        SessionMode::Stateless,
        &["/$diagnostics/**"],
    )
    .await;
    let mut app = server.app("inspector");
    receive(app.reconnect()).await.unwrap();
    let sequence = producer.publish("/tracked/temperature", 21).await;
    through(&mut app, sequence).await;
    server.now.fetch_add(3600, Ordering::SeqCst);
    producer.publish("/clock-tick", 1).await;
    let diagnostic = path("/$diagnostics/freshness/tracked/temperature");
    receive(async {
        while app.model().node(&diagnostic).is_none() {
            app.next_update().await.unwrap();
        }
    })
    .await;
    // The explicit listener sees the same reserved node in an ordinary update.
    receive(async {
        loop {
            if let Some(ServerMessage::Update { changes, .. }) =
                explicit.connection.recv().await.unwrap()
                && changes.iter().any(
                    |change| matches!(change, ChangeView::Upsert { topic, .. } if topic == &diagnostic),
                )
            {
                return;
            }
        }
    })
    .await;
    assert!(app.model().node(&diagnostic).is_some());
    // A fresh /** baseline contains the same reserved node too.
    let mut populated = server.app("second inspector");
    receive(populated.reconnect()).await.unwrap();
    assert!(populated.model().node(&diagnostic).is_some());
    app.close().await.unwrap();
    populated.close().await.unwrap();
    producer.close().await;
    explicit.close().await;
    server.stop().await;
}

#[tokio::test]
async fn a_same_named_managed_session_does_not_replace_the_stateless_inspector() {
    let server = Server::start().await;
    let mut producer = server.producer().await;
    producer.publish("/old", 7).await;
    let mut app = server.app("duplicate inspector");
    receive(app.reconnect()).await.unwrap();
    assert_eq!(value(&app, "/old"), 7);
    // A controller sharing the inspector's name keeps its managed session, and
    // the inspector keeps receiving updates.
    let namesake = Writer::connect(
        &server.url,
        "duplicate inspector",
        SessionMode::Managed,
        &[],
    )
    .await;
    producer
        .write(vec![WireOperation::RemoveNode {
            topic: path("/old"),
        }])
        .await;
    let sequence = producer.publish("/new", 8).await;
    through(&mut app, sequence).await;
    assert_eq!(app.model().status(), &ConnectionStatus::Live);
    assert!(app.model().node(&path("/old")).is_none());
    assert_eq!(value(&app, "/new"), 8);
    // An explicit reconnect installs an equivalent fresh baseline.
    receive(app.reconnect()).await.unwrap();
    assert_eq!(app.model().status(), &ConnectionStatus::Live);
    assert_eq!(value(&app, "/new"), 8);
    assert_eq!(server.core.lock().unwrap().managed_session_count(), 1);
    namesake.close().await;
    app.close().await.unwrap();
    producer.close().await;
    let core = server.core.clone();
    server.stop().await;
    assert_eq!(core.lock().unwrap().managed_session_count(), 0);
}

#[tokio::test]
async fn shutdown_and_failed_reconnect_preserve_stale_display() {
    let server = Server::start().await;
    let mut producer = server.producer().await;
    producer.publish("/old", 7).await;
    let mut app = server.app("inspector");
    receive(app.reconnect()).await.unwrap();
    producer.close().await;
    server.stop().await;
    // Shutdown may deliver a close frame or a transport reset. Both must
    // invalidate currentness; the inspector must preserve the actual reason.
    match receive(app.next_update()).await {
        Ok(()) => assert_eq!(app.model().status(), &ConnectionStatus::Disconnected),
        Err(AppError::Connection(_)) => {
            assert!(matches!(app.model().status(), ConnectionStatus::Error(_)))
        }
        Err(error) => panic!("unexpected shutdown error: {error}"),
    }
    assert_eq!(value(&app, "/old"), 7);
    assert!(receive(app.reconnect()).await.is_err());
    assert!(matches!(app.model().status(), ConnectionStatus::Error(_)));
    assert_eq!(value(&app, "/old"), 7);
    app.close().await.unwrap();
}

#[tokio::test]
async fn generated_names_allow_multiple_inspectors_without_replacement() {
    let server = Server::start().await;
    let first = ConnectionConfig::default();
    let second = ConnectionConfig::default();
    assert_ne!(first.client(), second.client());
    let mut a = App::new(ConnectionConfig::new(
        server.url.clone(),
        first.client().clone(),
    ));
    let mut b = App::new(ConnectionConfig::new(
        server.url.clone(),
        second.client().clone(),
    ));
    receive(a.reconnect()).await.unwrap();
    receive(b.reconnect()).await.unwrap();
    let mut producer = server.producer().await;
    let sequence = producer.publish("/both", 1).await;
    through(&mut a, sequence).await;
    through(&mut b, sequence).await;
    assert_eq!(value(&a, "/both"), 1);
    assert_eq!(value(&b, "/both"), 1);
    a.close().await.unwrap();
    b.close().await.unwrap();
    producer.close().await;
    server.stop().await;
}

#[tokio::test]
async fn cancelled_idle_receive_and_delayed_rendering_do_not_skip_batches() {
    let server = Server::start().await;
    let mut producer = server.producer().await;
    let mut app = server.app("inspector");
    receive(app.reconnect()).await.unwrap();
    tokio::select! { biased;
        _ = app.next_update() => panic!("no update yet"),
        () = std::future::ready(()) => {}
    }
    let first = producer.publish("/counter", 1).await;
    let second = producer.publish("/counter", 2).await;
    receive(app.next_update()).await.unwrap();
    assert_eq!(app.model().sequence(), first);
    assert_eq!(value(&app, "/counter"), 1);
    receive(app.next_update()).await.unwrap();
    assert_eq!(app.model().sequence(), second);
    assert_eq!(value(&app, "/counter"), 2);
    app.close().await.unwrap();
    producer.close().await;
    server.stop().await;
}

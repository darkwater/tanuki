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
use tanuki_client::{ExpiryUpdate, Session};
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
    async fn producer(&self, selectors: Vec<Selector>) -> Session {
        Session::connect(&self.url, ClientName::parse("producer").unwrap(), selectors)
            .await
            .unwrap()
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
    let Value::Integer(n) = current.value.as_inner() else {
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
    let producer = server.producer(vec![]).await;
    let receipt = producer
        .state::<i64>("/later/deep/value")
        .unwrap()
        .publish(&42, ExpiryUpdate::Clear)
        .await
        .unwrap();
    through(&mut app, receipt.sequence).await;
    assert_eq!(value(&app, "/later/deep/value"), 42);
    let mut populated = server.app("second inspector");
    receive(populated.reconnect()).await.unwrap();
    assert_eq!(value(&populated, "/later/deep/value"), 42);
    app.close().await.unwrap();
    populated.close().await.unwrap();
    producer.close().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn whole_batches_metadata_and_scheduled_expiry_reach_the_model() {
    let server = Server::start().await;
    let producer = server.producer(vec![]).await;
    let mut app = server.app("inspector");
    receive(app.reconnect()).await.unwrap();
    let hue = producer.state::<i64>("/lamp/hue").unwrap();
    let brightness = producer.state::<i64>("/lamp/brightness").unwrap();
    let mut batch = producer.batch();
    batch.publish(&hue, &120, ExpiryUpdate::Clear).unwrap();
    batch
        .publish(&brightness, &70, ExpiryUpdate::Clear)
        .unwrap();
    let receipt = batch.commit().await.unwrap();
    receive(app.next_update()).await.unwrap();
    assert_eq!(app.model().sequence(), receipt.sequence);
    assert_eq!(
        (value(&app, "/lamp/hue"), value(&app, "/lamp/brightness")),
        (120, 70)
    );
    let before = app.model().node(&path("/lamp/hue")).unwrap().clone();
    server.now.fetch_add(1, Ordering::SeqCst);
    let receipt = hue
        .publish(
            &120,
            ExpiryUpdate::Set(
                NonNegativeDuration::new(jiff::SignedDuration::from_secs(3600)).unwrap(),
            ),
        )
        .await
        .unwrap();
    through(&mut app, receipt.sequence).await;
    assert_ne!(app.model().node(&path("/lamp/hue")), Some(&before));
    assert_eq!(value(&app, "/lamp/hue"), 120);
    server.now.fetch_add(3600, Ordering::SeqCst);
    // A real transport mutation wakes the production scheduler after advancing
    // the injected wall clock. No timer sleeps or mock expiry behavior.
    producer
        .state::<i64>("/clock-tick")
        .unwrap()
        .publish(&1, ExpiryUpdate::Clear)
        .await
        .unwrap();
    receive(async {
        while app.model().node(&path("/lamp/hue")).is_some() {
            app.next_update().await.unwrap();
        }
    })
    .await;
    assert_eq!(value(&app, "/lamp/brightness"), 70);
    app.close().await.unwrap();
    producer.close().await.unwrap();
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
    let producer = server
        .producer(vec![Selector::parse("/$diagnostics/**").unwrap()])
        .await;
    let mut explicit = producer.listen_updates().await.unwrap();
    let mut app = server.app("inspector");
    receive(app.reconnect()).await.unwrap();
    let receipt = producer
        .state::<i64>("/tracked/temperature")
        .unwrap()
        .publish(&21, ExpiryUpdate::Clear)
        .await
        .unwrap();
    through(&mut app, receipt.sequence).await;
    server.now.fetch_add(3600, Ordering::SeqCst);
    producer
        .state::<i64>("/clock-tick")
        .unwrap()
        .publish(&1, ExpiryUpdate::Clear)
        .await
        .unwrap();
    let diagnostic = path("/$diagnostics/freshness/tracked/temperature");
    receive(async {
        while app.model().node(&diagnostic).is_none() {
            app.next_update().await.unwrap();
        }
    })
    .await;
    let batch = receive(explicit.recv()).await.unwrap().unwrap();
    assert!(batch.changes.iter().any(|change| matches!(change, ChangeView::Upsert { topic, .. } if topic == &diagnostic.to_string())));
    assert!(app.model().node(&diagnostic).is_some());
    // A fresh /** baseline contains the same reserved node too.
    let mut populated = server.app("second inspector");
    receive(populated.reconnect()).await.unwrap();
    assert!(populated.model().node(&diagnostic).is_some());
    app.close().await.unwrap();
    populated.close().await.unwrap();
    producer.close().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn duplicate_replacement_marks_cache_stale_and_reconnect_replaces_baseline() {
    let server = Server::start().await;
    let producer = server.producer(vec![]).await;
    let receipt = producer
        .state::<i64>("/old")
        .unwrap()
        .publish(&7, ExpiryUpdate::Clear)
        .await
        .unwrap();
    let mut app = server.app("duplicate inspector");
    receive(app.reconnect()).await.unwrap();
    assert_eq!(value(&app, "/old"), 7);
    assert!(app.model().sequence() >= receipt.sequence);
    let replacement = Session::connect(
        &server.url,
        ClientName::parse("duplicate inspector").unwrap(),
        vec![],
    )
    .await
    .unwrap();
    assert!(receive(app.next_update()).await.is_err());
    assert!(matches!(app.model().status(), ConnectionStatus::Error(_)));
    assert_eq!(value(&app, "/old"), 7);
    producer
        .write(vec![WireOperation::RemoveNode {
            topic: path("/old"),
        }])
        .await
        .unwrap();
    producer
        .state::<i64>("/new")
        .unwrap()
        .publish(&8, ExpiryUpdate::Clear)
        .await
        .unwrap();
    receive(app.reconnect()).await.unwrap();
    assert_eq!(app.model().status(), &ConnectionStatus::Live);
    assert!(app.model().node(&path("/old")).is_none());
    assert_eq!(value(&app, "/new"), 8);
    let _ = replacement.close().await;
    app.close().await.unwrap();
    producer.close().await.unwrap();
    let core = server.core.clone();
    server.stop().await;
    assert_eq!(core.lock().unwrap().managed_session_count(), 0);
}

#[tokio::test]
async fn shutdown_and_failed_reconnect_preserve_stale_display_and_join_session() {
    let server = Server::start().await;
    let producer = server.producer(vec![]).await;
    producer
        .state::<i64>("/old")
        .unwrap()
        .publish(&7, ExpiryUpdate::Clear)
        .await
        .unwrap();
    let mut app = server.app("inspector");
    receive(app.reconnect()).await.unwrap();
    producer.close().await.unwrap();
    server.stop().await;
    // Shutdown may deliver a close frame or a transport reset. Both must
    // invalidate currentness; the inspector must preserve the actual SDK reason.
    match receive(app.next_update()).await {
        Ok(()) => assert_eq!(app.model().status(), &ConnectionStatus::Disconnected),
        Err(AppError::Ended(_)) => {
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
    let producer = server.producer(vec![]).await;
    let receipt = producer
        .state::<i64>("/both")
        .unwrap()
        .publish(&1, ExpiryUpdate::Clear)
        .await
        .unwrap();
    through(&mut a, receipt.sequence).await;
    through(&mut b, receipt.sequence).await;
    assert_eq!(value(&a, "/both"), 1);
    assert_eq!(value(&b, "/both"), 1);
    a.close().await.unwrap();
    b.close().await.unwrap();
    producer.close().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn cancelled_idle_receive_and_delayed_rendering_do_not_skip_batches() {
    let server = Server::start().await;
    let producer = server.producer(vec![]).await;
    let mut app = server.app("inspector");
    receive(app.reconnect()).await.unwrap();
    tokio::select! { biased;
        _ = app.next_update() => panic!("no update yet"),
        () = std::future::ready(()) => {}
    }
    let topic = producer.state::<i64>("/counter").unwrap();
    let first = topic.publish(&1, ExpiryUpdate::Clear).await.unwrap();
    let second = topic.publish(&2, ExpiryUpdate::Clear).await.unwrap();
    receive(app.next_update()).await.unwrap();
    assert_eq!(app.model().sequence(), first.sequence);
    assert_eq!(value(&app, "/counter"), 1);
    receive(app.next_update()).await.unwrap();
    assert_eq!(app.model().sequence(), second.sequence);
    assert_eq!(value(&app, "/counter"), 2);
    app.close().await.unwrap();
    producer.close().await.unwrap();
    server.stop().await;
}

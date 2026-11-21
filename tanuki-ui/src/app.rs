//! SDK ownership for a single live inspector. No rendering or extra update queue.
use std::{
    future::pending,
    sync::atomic::{AtomicU64, Ordering},
};

use tokio::{sync::oneshot, task::JoinHandle};

use tanuki_client::{ClientError, Closed, Event, Listener, Reconnect, Resync, Session, Status};
use tanuki_protocol::{ClientName, SnapshotView};

use crate::model::{ConnectionStatus, Model, ModelError};

pub const DEFAULT_URL: &str = "ws://127.0.0.1:5167/v1/ws";

#[derive(Clone, Debug)]
pub struct ConnectionConfig {
    url: String,
    client: ClientName,
}
impl ConnectionConfig {
    pub fn new(url: String, client: ClientName) -> Self {
        Self { url, client }
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn client(&self) -> &ClientName {
        &self.client
    }
}
impl Default for ConnectionConfig {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        // Stateless connections never replace each other, so the name is only
        // attribution; PID and counter keep simultaneous inspectors apart in logs.
        let name = format!(
            "tanuki-ui-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        Self::new(
            DEFAULT_URL.into(),
            ClientName::parse(&name).expect("generated valid client name"),
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("inspector connection task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("session ended: {0}")]
    Ended(Closed),
    #[error(transparent)]
    Model(#[from] ModelError),
}

struct LiveConnection {
    session: Session,
    // Removed once the stream ends or fails; the session stays owned until
    // joined during explicit reconnect/exit.
    listener: Option<Listener>,
    baseline: SnapshotView,
}

struct ConnectionAttempt {
    cancel: oneshot::Sender<()>,
    task: JoinHandle<Result<Option<LiveConnection>, AppError>>,
}

pub struct App {
    config: ConnectionConfig,
    model: Model,
    connection: Option<LiveConnection>,
    attempt: Option<ConnectionAttempt>,
}
impl App {
    pub fn new(config: ConnectionConfig) -> Self {
        let mut model = Model::from_snapshot(SnapshotView {
            sequence: 0,
            nodes: Default::default(),
        })
        .expect("empty valid snapshot");
        model.set_status(ConnectionStatus::Disconnected);
        Self {
            config,
            model,
            connection: None,
            attempt: None,
        }
    }
    pub fn config(&self) -> &ConnectionConfig {
        &self.config
    }
    pub fn model(&self) -> &Model {
        &self.model
    }
    pub fn model_mut(&mut self) -> &mut Model {
        &mut self.model
    }

    /// Start one bounded connection job. Repeated reconnect input while connecting
    /// is ignored. The job owns cleanup; it never queues updates or draws.
    pub fn begin_reconnect(&mut self) {
        if self.attempt.is_some() {
            return;
        }
        self.model.set_status(ConnectionStatus::Connecting);
        let config = self.config.clone();
        let previous = self.connection.take();
        let (cancel, cancelled) = oneshot::channel();
        self.attempt = Some(ConnectionAttempt {
            cancel,
            task: tokio::spawn(open(config, previous, cancelled)),
        });
    }

    /// Convenience for non-terminal callers. The terminal uses begin_reconnect
    /// and next_update so it can keep rendering/input active during handshakes.
    pub async fn reconnect(&mut self) -> Result<(), AppError> {
        self.begin_reconnect();
        self.next_update().await
    }

    fn install(&mut self, connection: LiveConnection) -> Result<(), AppError> {
        // The listener's own baseline, not hello's, precedes its updates.
        let result = self
            .model
            .replace_snapshot(connection.baseline.clone())
            .map_err(AppError::from);
        self.connection = Some(connection);
        match &result {
            Ok(()) => self.model.set_status(ConnectionStatus::Live),
            Err(_) => self.drop_listener(),
        }
        result
    }

    fn drop_listener(&mut self) {
        if let Some(connection) = &mut self.connection {
            connection.listener = None;
        }
    }

    /// Apply exactly one complete batch or baseline. Suitable as a branch of
    /// tokio::select!; cancelling the receive does not dequeue data. With no
    /// active listener, remain pending so an input/tick loop cannot spin or
    /// auto-reconnect.
    pub async fn next_update(&mut self) -> Result<(), AppError> {
        if let Some(attempt) = &mut self.attempt {
            // Awaiting a borrowed JoinHandle is cancellation safe: a render tick
            // may cancel this poll without losing a completed connection.
            let joined = (&mut attempt.task).await;
            self.attempt = None;
            let result = match joined {
                Ok(Ok(Some(connection))) => self.install(connection),
                Ok(Ok(None)) => {
                    self.model.set_status(ConnectionStatus::Disconnected);
                    Ok(())
                }
                Ok(Err(error)) => Err(error),
                Err(error) => Err(error.into()),
            };
            if let Err(error) = &result {
                self.model
                    .set_status(ConnectionStatus::Error(error.to_string()));
                tracing::warn!(error=%crate::format::safe_text(&error.to_string()),"inspector connection attempt failed; cache is stale");
            }
            return result;
        }
        let Some(connection) = self.connection.as_mut() else {
            return pending().await;
        };
        let Some(listener) = connection.listener.as_mut() else {
            return pending().await;
        };
        let received = listener.recv().await;
        let ended = match connection.session.status().current() {
            Status::Closed(closed) => Some(closed),
            _ => None,
        };
        self.accept_received(received, ended)
    }

    // This boundary also permits deterministic injection of SDK results in unit
    // tests. The SDK's own tests cover queue delivery and lag resynchronisation.
    fn accept_received(
        &mut self,
        received: Option<Event>,
        ended: Option<Closed>,
    ) -> Result<(), AppError> {
        let result = match received {
            Some(Event::Update(update)) => self.model.apply(update).map_err(AppError::from),
            Some(Event::Snapshot { snapshot, reason }) => {
                if reason != Resync::Started {
                    tracing::warn!(?reason, "inspector resynchronised from a fresh baseline");
                }
                self.model
                    .replace_snapshot(SnapshotView::from(&snapshot))
                    .map_err(AppError::from)
            }
            None => match ended {
                Some(Closed::Failed(cause)) => Err(AppError::Ended(Closed::Failed(cause))),
                _ => {
                    self.model.set_status(ConnectionStatus::Disconnected);
                    Ok(())
                }
            },
        };
        if let Err(error) = &result {
            self.model
                .set_status(ConnectionStatus::Error(error.to_string()));
            tracing::warn!(error = %crate::format::safe_text(&error.to_string()), "inspector stream ended; cached view is stale");
        }
        if result.is_err() || matches!(self.model.status(), ConnectionStatus::Disconnected) {
            self.drop_listener();
        }
        result
    }

    /// Always await on exit. Dropping App signals SDK shutdown as a fallback,
    /// but only this explicit method joins the session's background task.
    pub async fn close(&mut self) -> Result<(), AppError> {
        self.model.set_status(ConnectionStatus::Disconnected);
        if let Some(attempt) = self.attempt.take() {
            let _ = attempt.cancel.send(());
            if let Some(connection) = attempt.task.await?? {
                connection.session.close().await;
            }
        }
        if let Some(connection) = self.connection.take() {
            drop(connection.listener);
            connection.session.close().await;
        }
        Ok(())
    }
}

/// Cancellation is checked during connect and registration. Every Session
/// obtained by the job is either returned to App or closed/joined by this job.
/// The inspector connects statelessly: it never replaces a same-named client's
/// managed session (and with it any input claims). It reconnects only when
/// asked, so the display keeps an explicit, labeled stale state.
async fn open(
    config: ConnectionConfig,
    previous: Option<LiveConnection>,
    mut cancelled: oneshot::Receiver<()>,
) -> Result<Option<LiveConnection>, AppError> {
    if let Some(previous) = previous {
        drop(previous.listener);
        previous.session.close().await;
    }
    let connect = Session::builder(config.url(), config.client.as_str())
        .stateless()
        .reconnect(Reconnect::Never)
        .connect();
    let session = tokio::select! {biased;
        _=&mut cancelled=>return Ok(None),
        session=connect=>session?,
    };
    let mut listener = match session.listen("/**") {
        Ok(listener) => listener,
        Err(error) => {
            session.close().await;
            return Err(error.into());
        }
    };
    let first = tokio::select! {biased;
        _=&mut cancelled=>None,
        event=listener.recv()=>event,
    };
    match first {
        Some(Event::Snapshot { snapshot, .. }) => Ok(Some(LiveConnection {
            session,
            listener: Some(listener),
            baseline: SnapshotView::from(&snapshot),
        })),
        _ => {
            let ended = session.status().current();
            session.close().await;
            match ended {
                Status::Closed(Closed::Failed(cause)) => {
                    Err(AppError::Ended(Closed::Failed(cause)))
                }
                _ => Ok(None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tanuki_client::{Disconnect, Snapshot};
    use tanuki_protocol::{
        ChangeView, ClientViewError, CurrentValueView, NodeView, ProvenanceView, TopicPath,
        UpdateView, Value, WireValue,
    };

    fn baseline(sequence: u64) -> SnapshotView {
        SnapshotView {
            sequence,
            nodes: [(
                "/cached".parse().unwrap(),
                NodeView::State {
                    current: CurrentValueView {
                        value: WireValue::new(Value::Integer(42)),
                        last_write: ProvenanceView {
                            client: "writer".parse().unwrap(),
                            session: None,
                            at: "2026-10-31T00:00:00Z".parse().unwrap(),
                        },
                        expires_at: None,
                    },
                },
            )]
            .into(),
        }
    }

    #[test]
    fn baselines_replace_data_and_a_failed_end_keeps_stale_data_with_its_cause() {
        let mut app = App::new(ConnectionConfig::default());
        app.model.replace_snapshot(baseline(10)).unwrap();
        app.model.set_status(ConnectionStatus::Live);
        // A lagged listener is resynchronised rather than ended.
        let empty = Snapshot::from(SnapshotView {
            sequence: 12,
            nodes: Default::default(),
        });
        app.accept_received(
            Some(Event::Snapshot {
                snapshot: empty,
                reason: Resync::Lagged,
            }),
            None,
        )
        .unwrap();
        assert_eq!(app.model.status(), &ConnectionStatus::Live);
        assert_eq!(app.model.sequence(), 12);
        app.model.replace_snapshot(baseline(13)).unwrap();

        let cause = Disconnect::Protocol("broken stream".into());
        let error = app
            .accept_received(None, Some(Closed::Failed(cause)))
            .unwrap_err();
        assert!(matches!(error, AppError::Ended(Closed::Failed(_))));
        assert_eq!(
            app.model.status(),
            &ConnectionStatus::Error("session ended: protocol error: broken stream".into())
        );
        assert!(
            app.model
                .node(&TopicPath::parse("/cached").unwrap())
                .is_some()
        );
        app.model.set_status(ConnectionStatus::Live);
        app.accept_received(None, Some(Closed::ByClient)).unwrap();
        assert_eq!(app.model.status(), &ConnectionStatus::Disconnected);
        assert_eq!(app.model.sequence(), 13);
    }

    #[test]
    fn failed_batch_application_ends_currentness_without_partial_data() {
        let mut app = App::new(ConnectionConfig::default());
        app.model.replace_snapshot(baseline(10)).unwrap();
        app.model.set_status(ConnectionStatus::Live);
        let previous = app
            .model
            .node(&TopicPath::parse("/cached").unwrap())
            .unwrap()
            .clone();
        let error = app
            .accept_received(
                Some(Event::Update(UpdateView {
                    sequence: 10,
                    changes: vec![ChangeView::Removed {
                        topic: "/cached".parse().unwrap(),
                        previous,
                    }],
                })),
                None,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            AppError::Model(ModelError::Sequence(ClientViewError::OutOfOrder { .. }))
        ));
        assert!(matches!(app.model.status(), ConnectionStatus::Error(_)));
        assert!(
            app.model
                .node(&TopicPath::parse("/cached").unwrap())
                .is_some()
        );
    }

    #[tokio::test]
    async fn idle_owner_is_pending_and_close_is_idempotent() {
        let mut app = App::new(ConnectionConfig::default());
        assert_eq!(app.config.url(), DEFAULT_URL);
        assert!(
            app.config
                .client
                .as_str()
                .starts_with(&format!("tanuki-ui-{}-", std::process::id()))
        );
        tokio::select! { biased;
            _ = app.next_update() => panic!("idle owner would spin"),
            () = std::future::ready(()) => {}
        }
        app.close().await.unwrap();
        app.close().await.unwrap();
        assert_eq!(app.model.status(), &ConnectionStatus::Disconnected);
    }

    #[tokio::test]
    async fn the_inspector_is_stateless_and_reconnect_installs_a_current_baseline() {
        use std::{
            sync::{Arc, Mutex},
            time::Duration,
        };
        use tanuki::{core::Core, domain::Timestamp, server::serve_with_core};
        use tanuki_client::{ClaimRelease, DesiredTopic, StateTopic};
        use tokio::{net::TcpListener, sync::oneshot, time::timeout};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/v1/ws", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(serve_with_core(
            listener,
            Arc::new(Mutex::new(Core::new())),
            Arc::new(|| Timestamp::new(jiff::Timestamp::now())),
            async {
                let _ = stopped.await;
            },
        ));
        // A controller with the same name as the inspector keeps its claim.
        let controller = Session::connect(&url, "inspector").await.unwrap();
        let desired = DesiredTopic::<u8>::new("/lamp/desired").unwrap();
        let mut control = controller
            .control(&desired, ClaimRelease::Immediate)
            .await
            .unwrap();
        let config = ConnectionConfig::new(url.clone(), ClientName::parse("inspector").unwrap());
        let mut app = App::new(config);
        timeout(Duration::from_secs(5), app.reconnect())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(app.model.status(), &ConnectionStatus::Live);
        assert!(matches!(controller.status().current(), Status::Connected));

        let producer = Session::connect(&url, "producer").await.unwrap();
        let topic = StateTopic::<i64>::new("/after-connect").unwrap();
        let receipt = producer.publish(&topic, &5).await.unwrap();
        while app.model.sequence() < receipt.sequence {
            timeout(Duration::from_secs(5), app.next_update())
                .await
                .unwrap()
                .unwrap();
        }
        assert!(app.model.node(topic.path()).is_some());
        app.model.set_filter("after".into());
        // An explicit reconnect installs a fresh baseline and keeps navigation.
        timeout(Duration::from_secs(5), app.reconnect())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(app.model.status(), &ConnectionStatus::Live);
        assert_eq!(app.model.filter(), "after");
        assert!(app.model.node(topic.path()).is_some());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), control.recv())
                .await
                .is_err(),
            "the controller kept its claim"
        );
        app.close().await.unwrap();
        producer.close().await;
        controller.close().await;
        let _ = stop.send(());
        timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

#[cfg(test)]
mod connecting_tests {
    use super::*;
    #[tokio::test]
    async fn stalled_handshake_can_be_drawn_and_cancelled_with_joined_exit() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = ConnectionConfig::new(
            format!("ws://{}", listener.local_addr().unwrap()),
            ClientName::parse("inspector").unwrap(),
        );
        let mut app = App::new(config);
        app.begin_reconnect();
        assert_eq!(app.model().status(), &ConnectionStatus::Connecting);
        // Prove the job has sent a real handshake before testing cancellation.
        // The peer deliberately never answers it; there are no timing sleeps.
        use tokio::io::AsyncReadExt;
        let (mut peer, _) =
            tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                .await
                .unwrap()
                .unwrap();
        let mut byte = [0; 1];
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), peer.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            1
        );
        tokio::select! {biased;
            _=app.next_update()=>panic!("handshake cannot complete without a peer"),
            ()=std::future::ready(())=>{}
        }
        // Drawing/input may borrow the model between cancelled receive polls.
        assert_eq!(app.model().status(), &ConnectionStatus::Connecting);
        tokio::time::timeout(std::time::Duration::from_secs(1), app.close())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(app.model().status(), &ConnectionStatus::Disconnected);
    }
}

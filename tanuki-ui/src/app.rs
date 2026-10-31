//! SDK ownership for a single live inspector. No rendering or extra update queue.
use std::{
    future::pending,
    sync::atomic::{AtomicU64, Ordering},
};

use tokio::{sync::oneshot, task::JoinHandle};

use tanuki_client::{ClientError, RawListener, Session, SessionEnd};
use tanuki_protocol::{ClientName, Selector, SnapshotView, UpdateView};

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
        // PID separates simultaneous local processes; the counter also separates
        // inspectors constructed inside one process. Attribution, not authentication.
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
    #[error(transparent)]
    Ended(#[from] SessionEnd),
    #[error(transparent)]
    Model(#[from] ModelError),
}

struct LiveConnection {
    session: Session,
    // A terminal listener is removed, but the session stays owned until joined
    // during explicit reconnect/exit. In particular, lag never registers again.
    listener: Option<RawListener>,
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

    fn install(&mut self, mut connection: LiveConnection) -> Result<(), AppError> {
        let snapshot = connection
            .listener
            .as_ref()
            .expect("new listener")
            .initial_snapshot()
            .clone();
        // Registration, not historical hello, is the baseline for this listener.
        let result = self
            .model
            .replace_snapshot(snapshot)
            .map_err(AppError::from);
        if result.is_err() {
            connection.listener = None;
        }
        self.connection = Some(connection); // Still own failed setup for joined close.
        if result.is_ok() {
            self.model.set_status(ConnectionStatus::Live);
        }
        result
    }

    #[cfg(test)]
    async fn install_session(&mut self, session: Session) -> Result<(), AppError> {
        let listener = session.listen_updates().await?;
        self.install(LiveConnection {
            session,
            listener: Some(listener),
        })
    }

    /// Apply exactly one complete batch. Suitable as a branch of tokio::select!;
    /// cancelling the receive does not dequeue data. With no active listener,
    /// remain pending so an input/tick loop cannot spin or auto-reconnect.
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
        let Some(listener) = self
            .connection
            .as_mut()
            .and_then(|connection| connection.listener.as_mut())
        else {
            return pending().await;
        };
        let received = listener.recv().await;
        self.accept_received(received)
    }

    // This boundary also permits deterministic injection of terminal SDK results
    // in unit tests. The SDK's own tests cover raw queue delivery and overflow.
    fn accept_received(
        &mut self,
        received: Result<Option<UpdateView>, SessionEnd>,
    ) -> Result<(), AppError> {
        let result = match received {
            Ok(Some(update)) => self.model.apply(update).map_err(AppError::from),
            Ok(None) => {
                self.model.set_status(ConnectionStatus::Disconnected);
                Ok(())
            }
            Err(SessionEnd::Lagged) => {
                self.model.set_status(ConnectionStatus::Lagged);
                Err(SessionEnd::Lagged.into())
            }
            Err(error) => Err(error.into()),
        };
        if let Err(error) = &result {
            if !matches!(self.model.status(), ConnectionStatus::Lagged) {
                self.model
                    .set_status(ConnectionStatus::Error(error.to_string()));
            }
            tracing::warn!(error = %crate::format::safe_text(&error.to_string()), "inspector stream ended; cached view is stale");
        }
        if (result.is_err() || matches!(self.model.status(), ConnectionStatus::Disconnected))
            && let Some(connection) = &mut self.connection
        {
            connection.listener = None;
        }
        result
    }

    /// Always await on exit. Dropping App signals SDK shutdown as a fallback,
    /// but only this explicit method joins the session's driver/writer tasks.
    pub async fn close(&mut self) -> Result<(), AppError> {
        self.model.set_status(ConnectionStatus::Disconnected);
        if let Some(attempt) = self.attempt.take() {
            let _ = attempt.cancel.send(());
            if let Some(connection) = attempt.task.await?? {
                connection.session.close().await?;
            }
        }
        if let Some(connection) = self.connection.take() {
            drop(connection.listener);
            connection.session.close().await?;
        }
        Ok(())
    }
}

/// Cancellation is checked before connect and during registration. Every Session
/// obtained by the job is either returned to App or closed/joined by this job.
async fn open(
    config: ConnectionConfig,
    previous: Option<LiveConnection>,
    mut cancelled: oneshot::Receiver<()>,
) -> Result<Option<LiveConnection>, AppError> {
    if let Some(previous) = previous {
        drop(previous.listener);
        if let Err(error) = previous.session.close().await {
            tracing::warn!(error=%crate::format::safe_text(&error.to_string()),"previous inspector session joined before reconnect");
        }
    }
    let session = tokio::select! {biased;
        _=&mut cancelled=>return Ok(None),
        session=Session::connect(config.url(),config.client.clone(),vec![Selector::parse("/**").expect("valid selection")])=>session?,
    };
    let listener = tokio::select! {biased;
        _=&mut cancelled=>None,
        listener=session.listen_updates()=>Some(listener),
    };
    match listener {
        Some(Ok(listener)) => {
            for warning in session.hello_warnings() {
                tracing::warn!(?warning, "inspector hello accepted with warning");
            }
            Ok(Some(LiveConnection {
                session,
                listener: Some(listener),
            }))
        }
        result => {
            let closed = session.close().await;
            if let Some(Err(error)) = result {
                return Err(error.into());
            }
            closed?;
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tanuki_protocol::{
        ChangeView, ClientViewError, CurrentValueView, JsonValue, NodeView, ProvenanceView,
        TopicPath, Value,
    };

    fn baseline(sequence: u64) -> SnapshotView {
        SnapshotView {
            sequence,
            nodes: [(
                "/cached".into(),
                NodeView::State {
                    current: CurrentValueView {
                        value: JsonValue::new(Value::Integer(42)),
                        last_write: ProvenanceView {
                            client: "writer".into(),
                            session: None,
                            at: "2026-10-31T00:00:00Z".into(),
                        },
                        expires_at: None,
                    },
                },
            )]
            .into(),
        }
    }

    #[test]
    fn terminal_receive_results_preserve_stale_data_and_typed_errors() {
        let mut app = App::new(ConnectionConfig::default());
        app.model.replace_snapshot(baseline(10)).unwrap();
        app.model.set_status(ConnectionStatus::Live);
        assert!(matches!(
            app.accept_received(Err(SessionEnd::Lagged)),
            Err(AppError::Ended(SessionEnd::Lagged))
        ));
        assert_eq!(app.model.status(), &ConnectionStatus::Lagged);
        assert!(
            app.model
                .node(&TopicPath::parse("/cached").unwrap())
                .is_some()
        );
        app.model.set_status(ConnectionStatus::Live);
        let error = app
            .accept_received(Err(SessionEnd::Protocol("broken stream".into())))
            .unwrap_err();
        assert!(matches!(error, AppError::Ended(SessionEnd::Protocol(_))));
        assert_eq!(
            app.model.status(),
            &ConnectionStatus::Error("protocol violation: broken stream".into())
        );
        assert_eq!(app.model.sequence(), 10);
        app.model.set_status(ConnectionStatus::Live);
        app.accept_received(Ok(None)).unwrap();
        assert_eq!(app.model.status(), &ConnectionStatus::Disconnected);
        assert_eq!(app.model.sequence(), 10);
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
            .accept_received(Ok(Some(UpdateView {
                sequence: 10,
                changes: vec![ChangeView::Removed {
                    topic: "/cached".into(),
                    previous,
                }],
            })))
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
    async fn registration_uses_current_listener_baseline_and_lag_requires_reconnect() {
        use std::{
            sync::{Arc, Mutex},
            time::Duration,
        };
        use tanuki::{core::Core, domain::Timestamp, server::serve_with_core};
        use tanuki_client::ExpiryUpdate;
        use tokio::{net::TcpListener, sync::oneshot, time::timeout};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/v1/ws", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(serve_with_core(
            listener,
            Arc::new(Mutex::new(Core::new())),
            Arc::new(|| Timestamp::new(jiff::Timestamp::from_second(1_700_000_000).unwrap())),
            async {
                let _ = stopped.await;
            },
        ));
        let config = ConnectionConfig::new(url.clone(), ClientName::parse("inspector").unwrap());
        let observer = Session::connect(
            &url,
            config.client.clone(),
            vec![Selector::parse("/**").unwrap()],
        )
        .await
        .unwrap();
        assert!(observer.initial_snapshot().nodes.is_empty());
        let mut barrier = observer.listen_updates().await.unwrap();
        let producer = Session::connect(&url, ClientName::parse("producer").unwrap(), vec![])
            .await
            .unwrap();
        let receipt = producer
            .state::<i64>("/between-hello-and-listen")
            .unwrap()
            .publish(&5, ExpiryUpdate::Clear)
            .await
            .unwrap();
        timeout(Duration::from_secs(5), barrier.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        // The barrier proves the observer driver has applied the intervening write.
        // A second registration must capture it even though hello did not.
        let mut app = App::new(config);
        app.install_session(observer).await.unwrap();
        let topic = TopicPath::parse("/between-hello-and-listen").unwrap();
        assert!(app.model.node(&topic).is_some());
        assert_eq!(app.model.sequence(), receipt.sequence);
        app.model.set_filter("between".into());
        let error = app.accept_received(Err(SessionEnd::Lagged)).unwrap_err();
        assert!(matches!(error, AppError::Ended(SessionEnd::Lagged)));
        assert_eq!(app.model.status(), &ConnectionStatus::Lagged);
        assert!(app.connection.as_ref().unwrap().listener.is_none());
        tokio::select! { biased;
            _ = app.next_update() => panic!("terminated listener must stay inactive"),
            () = std::future::ready(()) => {}
        }
        // Only an explicit reconnect may install another listener/baseline.
        app.reconnect().await.unwrap();
        assert_eq!(app.model.status(), &ConnectionStatus::Live);
        assert_eq!(app.model.filter(), "between");
        assert!(app.model.node(&topic).is_some());
        app.close().await.unwrap();
        producer.close().await.unwrap();
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

//! Connection ownership for a single live inspector. No rendering or extra
//! update queue: one stateless `/**` subscription whose hello snapshot is the
//! baseline and whose updates are complete commit batches.
use std::{
    future::pending,
    sync::atomic::{AtomicU64, Ordering},
};

use tokio::{sync::oneshot, task::JoinHandle};

use tanuki_client_core::{Connection, ConnectionError};
use tanuki_protocol::{
    ClientMessage, ClientName, ErrorView, RequestId, Selector, ServerMessage, SessionMode,
    SnapshotView, UpdateView,
};

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
    Connection(#[from] ConnectionError),
    #[error("server rejected the subscription: {} ({})", .0.message, .0.code)]
    Rejected(ErrorView),
    #[error("server answered hello with an unexpected message")]
    UnexpectedReply,
    #[error(transparent)]
    Model(#[from] ModelError),
}

struct ConnectionAttempt {
    cancel: oneshot::Sender<()>,
    task: JoinHandle<Result<Option<(Connection, SnapshotView)>, AppError>>,
}

pub struct App {
    config: ConnectionConfig,
    model: Model,
    // Present while the stream is live; dropped once it ends or fails.
    connection: Option<Connection>,
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

    fn install(&mut self, connection: Connection, baseline: SnapshotView) -> Result<(), AppError> {
        let result = self
            .model
            .replace_snapshot(baseline)
            .map_err(AppError::from);
        if result.is_ok() {
            self.connection = Some(connection);
            self.model.set_status(ConnectionStatus::Live);
        }
        result
    }

    /// Apply exactly one complete batch. Suitable as a branch of
    /// tokio::select!; cancelling the receive does not dequeue data. With no
    /// live connection, remain pending so an input/tick loop cannot spin or
    /// auto-reconnect.
    pub async fn next_update(&mut self) -> Result<(), AppError> {
        if let Some(attempt) = &mut self.attempt {
            // Awaiting a borrowed JoinHandle is cancellation safe: a render tick
            // may cancel this poll without losing a completed connection.
            let joined = (&mut attempt.task).await;
            self.attempt = None;
            let result = match joined {
                Ok(Ok(Some((connection, baseline)))) => self.install(connection, baseline),
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
        let received = connection.recv().await;
        self.accept_received(received)
    }

    // This boundary also permits deterministic injection of received messages
    // in unit tests.
    fn accept_received(
        &mut self,
        received: Result<Option<ServerMessage>, ConnectionError>,
    ) -> Result<(), AppError> {
        let result = match received {
            Ok(Some(ServerMessage::Update { sequence, changes })) => self
                .model
                .apply(UpdateView { sequence, changes })
                .map_err(AppError::from),
            Ok(Some(ServerMessage::Snapshot {
                sequence, nodes, ..
            })) => self
                .model
                .replace_snapshot(SnapshotView { sequence, nodes })
                .map_err(AppError::from),
            // The inspector sends nothing after hello, so nothing is correlated.
            Ok(Some(ServerMessage::Reply { .. })) => Ok(()),
            Ok(Some(ServerMessage::Error { error, .. })) => {
                tracing::warn!(code = %error.code, message = %crate::format::safe_text(&error.message), "server reported an error");
                Ok(())
            }
            Ok(None) => {
                self.model.set_status(ConnectionStatus::Disconnected);
                Ok(())
            }
            Err(error) => Err(error.into()),
        };
        if let Err(error) = &result {
            self.model
                .set_status(ConnectionStatus::Error(error.to_string()));
            tracing::warn!(error = %crate::format::safe_text(&error.to_string()), "inspector stream ended; cached view is stale");
        }
        if result.is_err() || matches!(self.model.status(), ConnectionStatus::Disconnected) {
            self.connection = None;
        }
        result
    }

    /// Await on exit to close the WebSocket normally.
    pub async fn close(&mut self) -> Result<(), AppError> {
        self.model.set_status(ConnectionStatus::Disconnected);
        if let Some(attempt) = self.attempt.take() {
            let _ = attempt.cancel.send(());
            if let Some((connection, _)) = attempt.task.await?? {
                let _ = connection.close().await;
            }
        }
        if let Some(connection) = self.connection.take() {
            let _ = connection.close().await;
        }
        Ok(())
    }
}

/// Cancellation is checked during connect and hello. Every connection opened
/// by the job is either returned to App or closed by this job. The inspector
/// connects statelessly: it never replaces a same-named client's managed
/// session (and with it any input claims). It reconnects only when asked, so
/// the display keeps an explicit, labeled stale state.
async fn open(
    config: ConnectionConfig,
    previous: Option<Connection>,
    mut cancelled: oneshot::Receiver<()>,
) -> Result<Option<(Connection, SnapshotView)>, AppError> {
    if let Some(previous) = previous {
        let _ = previous.close().await;
    }
    let mut connection = tokio::select! {biased;
        _ = &mut cancelled => return Ok(None),
        connection = Connection::open(config.url()) => connection?,
    };
    connection
        .send(&ClientMessage::Hello {
            request_id: RequestId::new("inspector".into()),
            client: config.client.clone(),
            selectors: vec![Selector::parse("/**").expect("a valid selector")],
            session: SessionMode::Stateless,
        })
        .await?;
    let first = tokio::select! {biased;
        _ = &mut cancelled => None,
        first = connection.recv() => Some(first),
    };
    let result = match first {
        None => Ok(None),
        Some(Ok(Some(ServerMessage::Snapshot {
            sequence, nodes, ..
        }))) => {
            return Ok(Some((connection, SnapshotView { sequence, nodes })));
        }
        Some(Ok(Some(ServerMessage::Error { error, .. }))) => Err(AppError::Rejected(error)),
        Some(Ok(Some(_))) => Err(AppError::UnexpectedReply),
        Some(Ok(None)) => Ok(None),
        Some(Err(error)) => Err(error.into()),
    };
    let _ = connection.close().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tanuki_protocol::{
        ChangeView, ClientViewError, CurrentValueView, NodeView, ProvenanceView, TopicPath, Value,
    };

    fn baseline(sequence: u64) -> SnapshotView {
        SnapshotView {
            sequence,
            nodes: [(
                "/cached".parse().unwrap(),
                NodeView::State {
                    current: CurrentValueView {
                        value: Value::Integer(42),
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

    fn cached(app: &App) -> bool {
        app.model
            .node(&TopicPath::parse("/cached").unwrap())
            .is_some()
    }

    #[test]
    fn a_failed_stream_keeps_stale_data_with_its_cause_and_a_clean_end_disconnects() {
        let mut app = App::new(ConnectionConfig::default());
        app.model.replace_snapshot(baseline(10)).unwrap();
        app.model.set_status(ConnectionStatus::Live);
        // Server-reported errors are logged; the stream continues.
        app.accept_received(Ok(Some(ServerMessage::Error {
            request_id: None,
            error: ErrorView {
                code: tanuki_protocol::ErrorCode::InternalError,
                message: "fixture".into(),
            },
        })))
        .unwrap();
        assert_eq!(app.model.status(), &ConnectionStatus::Live);

        let error = app
            .accept_received(Err(ConnectionError::Closed {
                code: 1013,
                reason: "slow_consumer".into(),
            }))
            .unwrap_err();
        assert!(matches!(error, AppError::Connection(_)));
        assert_eq!(
            app.model.status(),
            &ConnectionStatus::Error(
                "server closed WebSocket with code 1013: slow_consumer".into()
            )
        );
        assert!(cached(&app));

        app.model.set_status(ConnectionStatus::Live);
        app.accept_received(Ok(None)).unwrap();
        assert_eq!(app.model.status(), &ConnectionStatus::Disconnected);
        assert_eq!(app.model.sequence(), 10);
        assert!(cached(&app));
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
            .accept_received(Ok(Some(ServerMessage::Update {
                sequence: 10,
                changes: vec![ChangeView::Removed {
                    topic: "/cached".parse().unwrap(),
                    previous,
                }],
            })))
            .unwrap_err();
        assert!(matches!(
            error,
            AppError::Model(ModelError::Sequence(ClientViewError::OutOfOrder { .. }))
        ));
        assert!(matches!(app.model.status(), ConnectionStatus::Error(_)));
        assert!(cached(&app));
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

//! The shared session handle, its builder, status and write operations.
use crate::{
    Codec, ConnectionError, Controller, Listener, Observer, Occurrences, Topic, TopicKind,
    driver::{self, ClaimState, Command},
    listen::Queue,
    protocol::*,
    runtime::{self, Instant, JoinHandle},
    topic::{
        ClaimRelease, Desired, DurationOutOfRange, Event, Expiry, InputKind, InstantKind,
        IntoSelectors, State,
    },
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc, oneshot, watch};

/// How the session retries after losing its connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reconnect {
    /// End the session at the first disconnect.
    Never,
    /// Retry forever, waiting `initial`, doubling up to `max` between attempts.
    Backoff { initial: Duration, max: Duration },
}

impl Default for Reconnect {
    fn default() -> Self {
        Self::Backoff {
            initial: Duration::from_millis(500),
            max: Duration::from_secs(30),
        }
    }
}

/// Configures and opens a [`Session`]. Start from [`Session::builder`].
#[derive(Clone, Debug)]
pub struct SessionBuilder {
    pub(crate) url: String,
    pub(crate) client: String,
    pub(crate) mode: SessionMode,
    pub(crate) codec: Codec,
    pub(crate) reconnect: Reconnect,
    pub(crate) connect_timeout: Duration,
    pub(crate) write_timeout: Duration,
    pub(crate) max_pending_writes: usize,
    pub(crate) listener_capacity: usize,
}

impl SessionBuilder {
    /// Connect without a managed session: writes are attributed to the client
    /// name like one-off HTTP requests, inputs cannot be claimed, and other
    /// connections with the same name are never replaced. Suits dashboards,
    /// inspectors and browser tabs.
    #[must_use]
    pub fn stateless(mut self) -> Self {
        self.mode = SessionMode::Stateless;
        self
    }

    /// The codec for outgoing messages and subscriptions (JSON by default).
    #[must_use]
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }

    #[must_use]
    pub fn reconnect(mut self, reconnect: Reconnect) -> Self {
        self.reconnect = reconnect;
        self
    }

    /// Bound on opening the socket and completing hello (10 s by default).
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Bound on each write from call to reply (10 s by default).
    #[must_use]
    pub fn write_timeout(mut self, timeout: Duration) -> Self {
        self.write_timeout = timeout;
        self
    }

    /// Writes allowed in flight at once (64 by default); further writes wait.
    #[must_use]
    pub fn max_pending_writes(mut self, writes: usize) -> Self {
        self.max_pending_writes = writes.clamp(1, Semaphore::MAX_PERMITS);
        self
    }

    /// Events a [`Listener`] may fall behind by before it is resynchronised
    /// with a `Lagged` snapshot (256 by default).
    #[must_use]
    pub fn listener_capacity(mut self, events: usize) -> Self {
        self.listener_capacity = events.max(1);
        self
    }

    /// Connects now and fails if the first attempt does. Later disconnects
    /// follow the [`Reconnect`] policy.
    pub async fn connect(self) -> Result<Session, ClientError> {
        let session = self.launch(true)?;
        let mut status = session.status();
        loop {
            match status.current() {
                Status::Connected => return Ok(session),
                Status::Closed(Closed::Failed(cause)) => return Err(ClientError::Connect(cause)),
                Status::Closed(closed) => return Err(ClientError::Closed(closed)),
                Status::Connecting | Status::Reconnecting { .. } => {}
            }
            if status.changed().await.is_none() {
                return Err(ClientError::Closed(Closed::ByClient));
            }
        }
    }

    /// Returns immediately and connects in the background, retrying per the
    /// [`Reconnect`] policy from the first attempt. Watch [`Session::status`]
    /// to show connectivity; observers deliver once data arrives.
    pub fn start(self) -> Result<Session, ClientError> {
        self.launch(false)
    }

    fn launch(self, fail_first: bool) -> Result<Session, ClientError> {
        let client = ClientName::parse(&self.client)?;
        let (commands, receiver) = mpsc::unbounded_channel();
        let (status_sender, status) = watch::channel(Status::Connecting);
        let shared = Arc::new(Shared {
            commands,
            status,
            write_slots: Arc::new(Semaphore::new(self.max_pending_writes)),
            write_timeout: self.write_timeout,
            listener_capacity: self.listener_capacity,
            client: client.clone(),
            next_id: AtomicU64::new(1),
            driver: Mutex::new(None),
        });
        let task = runtime::spawn(driver::run(
            self,
            client,
            fail_first,
            receiver,
            status_sender,
        ));
        *shared.driver.lock().expect("driver handle lock") = Some(task);
        Ok(Session { shared })
    }
}

/// Connectivity of a [`Session`].
#[derive(Clone, Debug)]
pub enum Status {
    Connecting,
    Connected,
    /// Disconnected; the next attempt starts after `retry_in`.
    Reconnecting {
        attempt: u32,
        retry_in: Duration,
        cause: Disconnect,
    },
    /// The session has ended; observers and listeners have finished.
    Closed(Closed),
}

/// Why a connection attempt failed or a connection was lost.
#[derive(Clone, Debug, thiserror::Error)]
pub enum Disconnect {
    #[error(transparent)]
    Connection(Arc<ConnectionError>),
    #[error("server rejected hello: {} ({})", .0.message, .0.code)]
    Rejected(ErrorView),
    #[error("protocol error: {0}")]
    Protocol(Arc<str>),
    #[error("timed out")]
    Timeout,
}

/// Why a session ended.
#[derive(Clone, Debug, thiserror::Error)]
pub enum Closed {
    #[error("closed by the client")]
    ByClient,
    /// Another managed connection with the same client name took over. The
    /// session does not reconnect, which would take the name back.
    #[error("replaced by another managed connection with the same client name")]
    Replaced,
    #[error("{0}")]
    Failed(Disconnect),
}

/// Follows a session's [`Status`].
#[derive(Clone)]
pub struct StatusWatch(watch::Receiver<Status>);

impl StatusWatch {
    #[must_use]
    pub fn current(&self) -> Status {
        self.0.borrow().clone()
    }

    /// Waits for the next status change; `None` after the session ended.
    pub async fn changed(&mut self) -> Option<Status> {
        self.0.changed().await.ok()?;
        Some(self.0.borrow_and_update().clone())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("invalid client name: {0}")]
    InvalidName(#[from] ClientNameParseError),
    #[error("invalid selector: {0}")]
    InvalidSelector(#[from] SelectorParseError),
    #[error("invalid topic path: {0}")]
    InvalidPath(#[from] PathParseError),
    #[error(transparent)]
    Payload(#[from] PayloadError),
    #[error(transparent)]
    Duration(#[from] DurationOutOfRange),
    #[error("could not connect: {0}")]
    Connect(Disconnect),
    /// Not connected right now; nothing was sent. The session may reconnect.
    #[error("not connected")]
    Disconnected,
    #[error("session closed: {0}")]
    Closed(Closed),
    #[error("server rejected the request: {} ({})", .0.message, .0.code)]
    Rejected(ErrorView),
    /// The write limit stayed exhausted for the whole write timeout.
    #[error("too many writes in flight; nothing was sent")]
    Busy,
    #[error("connection lost after sending; the write may or may not have been applied")]
    OutcomeUnknown,
    #[error("no reply within the write timeout; the write may or may not have been applied")]
    ReplyTimeout,
    #[error("a write batch needs at least one operation")]
    EmptyBatch,
    #[error("invalid reply from server: {0}")]
    Reply(#[source] serde_json::Error),
    #[error("could not encode the request: {0}")]
    Encode(#[source] ConnectionError),
}

/// The server's acceptance of an atomic write.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct WriteReceipt {
    /// The commit sequence. Observers and listeners have already seen this
    /// commit when the receipt arrives, provided it touched their selectors.
    pub sequence: u64,
    pub warnings: Vec<DiagnosticView>,
}

/// A connection to a Tanuki server: cheap to clone and share. The session
/// reconnects per its [`Reconnect`] policy, keeps every [`Observer`],
/// [`Listener`] and [`Controller`] going across reconnects, and widens the
/// server-side selection as they are created. It ends on [`Session::close`],
/// when the last clone is dropped, or when replaced.
#[derive(Clone)]
pub struct Session {
    pub(crate) shared: Arc<Shared>,
}

pub(crate) struct Shared {
    pub(crate) commands: mpsc::UnboundedSender<Command>,
    status: watch::Receiver<Status>,
    write_slots: Arc<Semaphore>,
    write_timeout: Duration,
    listener_capacity: usize,
    client: ClientName,
    next_id: AtomicU64,
    driver: Mutex<Option<JoinHandle<()>>>,
}

impl Shared {
    pub(crate) fn unregister(&self, id: u64) {
        let _ = self.commands.send(Command::Unregister(id));
    }
    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

impl Session {
    /// Configures a session for the WebSocket `url` (such as
    /// `ws://127.0.0.1:5167/v1/ws`) and a client name, which attributes writes
    /// and, for managed sessions, identifies the session.
    #[must_use]
    pub fn builder(url: &str, client: &str) -> SessionBuilder {
        SessionBuilder {
            url: url.to_owned(),
            client: client.to_owned(),
            mode: SessionMode::Managed,
            codec: Codec::Json,
            reconnect: Reconnect::default(),
            connect_timeout: Duration::from_secs(10),
            write_timeout: Duration::from_secs(10),
            max_pending_writes: 64,
            listener_capacity: 256,
        }
    }

    /// Connects a managed session with default settings.
    pub async fn connect(url: &str, client: &str) -> Result<Self, ClientError> {
        Self::builder(url, client).connect().await
    }

    #[must_use]
    pub fn client(&self) -> &ClientName {
        &self.shared.client
    }

    #[must_use]
    pub fn status(&self) -> StatusWatch {
        StatusWatch(self.shared.status.clone())
    }

    /// Ends the session for every clone: closes the connection, finishes
    /// observers and listeners, and waits for the background task.
    pub async fn close(&self) {
        let _ = self.shared.commands.send(Command::Close);
        let task = self
            .shared
            .driver
            .lock()
            .expect("driver handle lock")
            .take();
        if let Some(task) = task
            && let Err(error) = task.await
        {
            tracing::warn!(%error, "Tanuki session task ended abnormally");
        }
    }

    /// Waits until the session has ended and returns why.
    pub async fn closed(&self) -> Closed {
        let mut status = self.status();
        loop {
            if let Status::Closed(closed) = status.current() {
                return closed;
            }
            if status.changed().await.is_none() {
                return Closed::ByClient;
            }
        }
    }

    fn check_open(&self) -> Result<(), ClientError> {
        match &*self.shared.status.borrow() {
            Status::Closed(closed) => Err(ClientError::Closed(closed.clone())),
            _ => Ok(()),
        }
    }

    /// Sends raw operations as one atomic batch. Prefer [`Session::batch`].
    pub async fn write(&self, operations: Vec<WireOperation>) -> Result<WriteReceipt, ClientError> {
        if operations.is_empty() {
            return Err(ClientError::EmptyBatch);
        }
        match &*self.shared.status.borrow() {
            Status::Connected => {}
            Status::Closed(closed) => return Err(ClientError::Closed(closed.clone())),
            Status::Connecting | Status::Reconnecting { .. } => {
                return Err(ClientError::Disconnected);
            }
        }
        let deadline = Instant::now() + self.shared.write_timeout;
        let _permit = runtime::timeout(
            self.shared.write_timeout,
            self.shared.write_slots.clone().acquire_owned(),
        )
        .await
        .map_err(|_| ClientError::Busy)?
        .map_err(|_| ClientError::Busy)?;
        let (reply, receiver) = oneshot::channel();
        self.shared
            .commands
            .send(Command::Write {
                operations,
                reply,
                deadline,
            })
            .map_err(|_| ClientError::Closed(Closed::ByClient))?;
        receiver.await.map_err(|_| ClientError::OutcomeUnknown)?
    }

    /// Starts an atomic batch of operations.
    pub fn batch(&self) -> Batch {
        Batch {
            session: self.clone(),
            operations: Vec::new(),
            error: None,
        }
    }

    /// Publishes a state value that stays until replaced.
    pub async fn publish<T: ToValue>(
        &self,
        topic: &Topic<T, State>,
        value: &T,
    ) -> Result<WriteReceipt, ClientError> {
        self.batch().publish(topic, value).commit().await
    }

    /// Publishes a state value with an explicit [`Expiry`], such as a lease
    /// that disappears unless republished.
    pub async fn publish_with<T: ToValue>(
        &self,
        topic: &Topic<T, State>,
        value: &T,
        expiry: Expiry,
    ) -> Result<WriteReceipt, ClientError> {
        self.batch()
            .publish_with(topic, value, expiry)
            .commit()
            .await
    }

    /// Emits one event occurrence.
    pub async fn emit<T: ToValue>(
        &self,
        topic: &Topic<T, Event>,
        value: &T,
    ) -> Result<WriteReceipt, ClientError> {
        self.batch().emit(topic, value).commit().await
    }

    /// Submits a desired request (which stays until replaced or cleared) or a
    /// command. The input must already be defined, normally by its controller;
    /// use a batch with [`Batch::define`] to create it yourself.
    pub async fn submit<T: ToValue, K: InputKind>(
        &self,
        topic: &Topic<T, K>,
        value: &T,
    ) -> Result<WriteReceipt, ClientError> {
        self.batch().submit(topic, value).commit().await
    }

    /// Submits a desired request with an explicit [`Expiry`].
    pub async fn submit_with<T: ToValue>(
        &self,
        topic: &Topic<T, Desired>,
        value: &T,
        expiry: Expiry,
    ) -> Result<WriteReceipt, ClientError> {
        self.batch()
            .submit_with(topic, value, expiry)
            .commit()
            .await
    }

    /// Withdraws the pending desired request, keeping the definition and claim.
    pub async fn clear<T>(&self, topic: &Topic<T, Desired>) -> Result<WriteReceipt, ClientError> {
        self.batch().clear(topic).commit().await
    }

    /// Removes a node of any kind.
    pub async fn remove<T, K: TopicKind>(
        &self,
        topic: &Topic<T, K>,
    ) -> Result<WriteReceipt, ClientError> {
        self.batch().remove(topic).commit().await
    }

    /// Observes the latest state of the selected topics: selector text such as
    /// `"/battery/*"`, a topic handle, or a collection of them. The session
    /// widens its server selection as needed.
    pub fn observe(&self, selectors: impl IntoSelectors) -> Result<Observer, ClientError> {
        self.check_open()?;
        let selectors: Arc<[Selector]> = selectors.into_selectors()?.into();
        let id = self.shared.next_id();
        let (sender, snapshots) = watch::channel(None);
        self.shared
            .commands
            .send(Command::Observe {
                id,
                selectors,
                sender,
            })
            .map_err(|_| ClientError::Closed(Closed::ByClient))?;
        Ok(Observer {
            id,
            snapshots,
            shared: Arc::downgrade(&self.shared),
            started: false,
        })
    }

    /// Receives every commit touching the selected topics, in order, including
    /// event and command occurrences.
    pub fn listen(&self, selectors: impl IntoSelectors) -> Result<Listener, ClientError> {
        self.check_open()?;
        let selectors: Arc<[Selector]> = selectors.into_selectors()?.into();
        let id = self.shared.next_id();
        let queue = Queue::new(self.shared.listener_capacity);
        self.shared
            .commands
            .send(Command::Listen {
                id,
                selectors,
                queue: Arc::clone(&queue),
            })
            .map_err(|_| ClientError::Closed(Closed::ByClient))?;
        Ok(Listener {
            id,
            queue,
            shared: Arc::downgrade(&self.shared),
        })
    }

    /// Receives each occurrence of an event or command topic, decoded.
    pub fn occurrences<T: FromValue, K: InstantKind>(
        &self,
        topic: &Topic<T, K>,
    ) -> Result<Occurrences<T, K>, ClientError> {
        Ok(Occurrences::new(self.listen(topic)?, topic))
    }

    /// Takes control of an input: defines it, claims it for this managed
    /// session, and returns a [`Controller`] delivering its requests. The
    /// session re-claims it after every reconnect. Fails if the server
    /// rejects the claim (for example from a stateless session).
    pub async fn control<T, K: InputKind>(
        &self,
        topic: &Topic<T, K>,
        release: ClaimRelease,
    ) -> Result<Controller<T, K>, ClientError>
    where
        T: FromValue,
    {
        self.check_open()?;
        let release = WireRelease::try_from(release)?;
        let id = self.shared.next_id();
        let (claim, claim_state) = watch::channel(ClaimState::Pending);
        let (reply, receiver) = oneshot::channel();
        // The controller's view of the topic, registered before claiming so
        // the claim's own update reaches it.
        let inner = match K::INPUT {
            WireInputKind::Desired => crate::control::Inner::Desired(self.observe(topic)?),
            WireInputKind::Command => crate::control::Inner::Command(self.listen(topic)?),
        };
        self.shared
            .commands
            .send(Command::Control {
                id,
                topic: topic.path().clone(),
                input: K::INPUT,
                release,
                claim,
                reply,
            })
            .map_err(|_| ClientError::Closed(Closed::ByClient))?;
        receiver
            .await
            .map_err(|_| ClientError::Closed(Closed::ByClient))??;
        Ok(Controller::new(
            id,
            topic,
            inner,
            claim_state,
            Arc::downgrade(&self.shared),
        ))
    }
}

/// An atomic batch: every operation commits together or not at all. Building
/// never fails; a payload that cannot be encoded is reported by `commit`.
/// Dropping a batch sends nothing.
#[must_use = "a batch does nothing until committed"]
pub struct Batch {
    session: Session,
    operations: Vec<WireOperation>,
    error: Option<ClientError>,
}

impl Batch {
    fn push(mut self, operation: impl FnOnce() -> Result<WireOperation, ClientError>) -> Self {
        if self.error.is_none() {
            match operation() {
                Ok(operation) => self.operations.push(operation),
                Err(error) => self.error = Some(error),
            }
        }
        self
    }

    pub fn publish<T: ToValue>(self, topic: &Topic<T, State>, value: &T) -> Self {
        self.publish_with(topic, value, Expiry::Never)
    }

    pub fn publish_with<T: ToValue>(
        self,
        topic: &Topic<T, State>,
        value: &T,
        expiry: Expiry,
    ) -> Self {
        self.push(|| {
            Ok(WireOperation::PublishState {
                topic: topic.path().clone(),
                value: WireValue::new(value.to_value()?),
                expiry: Some(expiry.try_into()?),
            })
        })
    }

    pub fn emit<T: ToValue>(self, topic: &Topic<T, Event>, value: &T) -> Self {
        self.push(|| {
            Ok(WireOperation::PublishEvent {
                topic: topic.path().clone(),
                value: WireValue::new(value.to_value()?),
            })
        })
    }

    pub fn submit<T: ToValue, K: InputKind>(self, topic: &Topic<T, K>, value: &T) -> Self {
        self.push(|| {
            let topic = topic.path().clone();
            let value = WireValue::new(value.to_value()?);
            Ok(match K::INPUT {
                WireInputKind::Desired => WireOperation::SubmitDesired {
                    topic,
                    value,
                    expiry: Some(WireExpiry::Clear),
                },
                WireInputKind::Command => WireOperation::SubmitCommand { topic, value },
            })
        })
    }

    pub fn submit_with<T: ToValue>(
        self,
        topic: &Topic<T, Desired>,
        value: &T,
        expiry: Expiry,
    ) -> Self {
        self.push(|| {
            Ok(WireOperation::SubmitDesired {
                topic: topic.path().clone(),
                value: WireValue::new(value.to_value()?),
                expiry: Some(expiry.try_into()?),
            })
        })
    }

    pub fn clear<T>(self, topic: &Topic<T, Desired>) -> Self {
        self.push(|| {
            Ok(WireOperation::ClearDesired {
                topic: topic.path().clone(),
            })
        })
    }

    /// Defines an input (keeping any pending request). Controllers normally
    /// do this through [`Session::control`].
    pub fn define<T, K: InputKind>(self, topic: &Topic<T, K>) -> Self {
        self.push(|| {
            Ok(WireOperation::DefineInput {
                topic: topic.path().clone(),
                kind: K::INPUT,
            })
        })
    }

    /// Claims an input once. Unlike [`Session::control`] the claim is not
    /// renewed after a reconnect.
    pub fn claim<T, K: InputKind>(self, topic: &Topic<T, K>, release: ClaimRelease) -> Self {
        self.push(|| {
            Ok(WireOperation::ClaimInput {
                topic: topic.path().clone(),
                release: release.try_into()?,
            })
        })
    }

    pub fn remove<T, K: TopicKind>(self, topic: &Topic<T, K>) -> Self {
        self.push(|| {
            Ok(WireOperation::RemoveNode {
                topic: topic.path().clone(),
            })
        })
    }

    /// Appends a raw operation.
    pub fn operation(self, operation: WireOperation) -> Self {
        self.push(|| Ok(operation))
    }

    pub async fn commit(self) -> Result<WriteReceipt, ClientError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        self.session.write(self.operations).await
    }
}

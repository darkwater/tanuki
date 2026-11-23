//! The reconnecting client: a cloneable [`Client`] for writing and an owned
//! [`Events`] stream for reading. A background task owns the connection.
use crate::{
    Codec, ConnectionError,
    protocol::{
        ClientName, CommitView, DiagnosticView, ErrorView, Selector, SessionMode, SnapshotView,
        UpdateView, WireOperation,
    },
    runtime,
};
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};

mod driver;
mod queue;

/// Configures and starts a [`Client`].
#[derive(Clone, Debug)]
pub struct ClientBuilder {
    pub(crate) url: String,
    pub(crate) name: ClientName,
    pub(crate) session: SessionMode,
    pub(crate) selectors: Vec<Selector>,
    pub(crate) setup: Vec<WireOperation>,
    pub(crate) codec: Codec,
    pub(crate) backoff: Backoff,
    pub(crate) connect_timeout: Duration,
    pub(crate) write_timeout: Duration,
    pub(crate) event_capacity: usize,
}

/// Reconnect delays: `initial`, doubling up to `max`, each randomly shortened
/// by up to half so that many clients do not reconnect in lockstep.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            initial: Duration::from_millis(500),
            max: Duration::from_secs(30),
        }
    }
}

impl ClientBuilder {
    /// A stateless session: attributed writes, but no claims, and it never
    /// replaces another connection with the same name.
    #[must_use]
    pub fn stateless(mut self) -> Self {
        self.session = SessionMode::Stateless;
        self
    }

    /// The initial selection. [`Client::select`] changes it later; every
    /// (re)connection's hello uses the latest one.
    #[must_use]
    pub fn selectors(mut self, selectors: impl IntoIterator<Item = Selector>) -> Self {
        self.selectors = selectors.into_iter().collect();
        self
    }

    /// Operations written as one batch right after every hello, before any
    /// other write: typically defining and claiming inputs, which a reconnect
    /// loses. See [`Client::set_setup`].
    #[must_use]
    pub fn setup(mut self, operations: Vec<WireOperation>) -> Self {
        self.setup = operations;
        self
    }

    /// The codec for outgoing messages, and so for the server's replies.
    #[must_use]
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }

    #[must_use]
    pub fn backoff(mut self, backoff: Backoff) -> Self {
        self.backoff = backoff;
        self
    }

    /// How long opening the socket and receiving the hello's snapshot may take
    /// (default 10 s).
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// How long [`Client::write`] waits for the server's reply (default 10 s).
    #[must_use]
    pub fn write_timeout(mut self, timeout: Duration) -> Self {
        self.write_timeout = timeout;
        self
    }

    /// Queued updates beyond this many are dropped and replaced by a fresh
    /// snapshot (default 1024). At least 1.
    #[must_use]
    pub fn event_capacity(mut self, updates: usize) -> Self {
        self.event_capacity = updates.max(1);
        self
    }

    /// Starts connecting in the background and returns immediately.
    ///
    /// Natively this spawns a Tokio task, so it must be called within a Tokio
    /// runtime; in the browser it uses `spawn_local`.
    pub fn start(self) -> (Client, Events) {
        let (commands, receiver) = mpsc::unbounded_channel();
        let (status_sender, status) = watch::channel(Status::Connecting);
        let queue = Arc::new(queue::Queue::new(self.event_capacity));
        let client = Client {
            commands,
            status,
            name: self.name.clone(),
            write_timeout: self.write_timeout,
        };
        let events = Events {
            queue: Arc::clone(&queue),
        };
        runtime::spawn(driver::run(self, receiver, status_sender, queue));
        (client, events)
    }
}

/// A cheap, cloneable handle for writing to and steering one connection.
///
/// The connection lives while any handle does: dropping the last `Client`
/// closes it, as does [`close`](Self::close).
#[derive(Clone, Debug)]
pub struct Client {
    commands: mpsc::UnboundedSender<driver::Command>,
    status: watch::Receiver<Status>,
    name: ClientName,
    write_timeout: Duration,
}

impl Client {
    /// Defaults: managed session, empty selection, JSON, 0.5–30 s backoff,
    /// 10 s connect and write timeouts.
    #[must_use]
    pub fn builder(url: impl Into<String>, name: ClientName) -> ClientBuilder {
        ClientBuilder {
            url: url.into(),
            name,
            session: SessionMode::Managed,
            selectors: Vec::new(),
            setup: Vec::new(),
            codec: Codec::Json,
            backoff: Backoff::default(),
            connect_timeout: Duration::from_secs(10),
            write_timeout: Duration::from_secs(10),
            event_capacity: 1024,
        }
    }

    #[must_use]
    pub fn name(&self) -> &ClientName {
        &self.name
    }

    #[must_use]
    pub fn status(&self) -> Status {
        *self.status.borrow()
    }

    /// Writes one atomic batch and waits for the server's reply.
    ///
    /// When this returns `Ok`, the batch's own [`Event::Update`] has already
    /// been queued on [`Events`] (unless the queue overflowed, in which case a
    /// resync snapshot including it follows). Fails at once with
    /// [`ClientError::NotConnected`] while disconnected; writes are never
    /// queued across reconnects. On a new connection the setup batch is sent
    /// before any write, so it commits first.
    pub async fn write(&self, operations: Vec<WireOperation>) -> Result<CommitView, ClientError> {
        let (reply, result) = oneshot::channel();
        self.send(driver::Command::Write { operations, reply })?;
        match runtime::timeout(self.write_timeout, result).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(ClientError::Closed),
            Err(runtime::Elapsed) => Err(ClientError::Timeout),
        }
    }

    /// Replaces the selection. While connected this resolves once the new
    /// selection's [`Event::Snapshot`] (cause [`SnapshotCause::Selected`]) has
    /// been queued; while disconnected it resolves at once and the next
    /// connection uses it.
    pub async fn select(&self, selectors: Vec<Selector>) -> Result<(), ClientError> {
        let (reply, result) = oneshot::channel();
        self.send(driver::Command::Select { selectors, reply })?;
        result.await.unwrap_or(Err(ClientError::Closed))
    }

    /// Replaces the setup batch for future connections. It is not written
    /// now: write the same operations yourself if the current connection
    /// needs them too.
    pub fn set_setup(&self, operations: Vec<WireOperation>) {
        let _ = self.send(driver::Command::SetSetup(operations));
    }

    /// Closes the connection normally and stops reconnecting. Returns once the
    /// background task has finished; [`Events`] ends with
    /// [`Event::Closed`]`(`[`Closed::Requested`]`)`.
    pub async fn close(&self) {
        let _ = self.send(driver::Command::Close);
        let mut status = self.status.clone();
        let _ = status.wait_for(|status| *status == Status::Closed).await;
    }

    fn send(&self, command: driver::Command) -> Result<(), ClientError> {
        self.commands.send(command).map_err(|_| ClientError::Closed)
    }
}

/// Everything the connection observes, in order. It must keep being read:
/// see [`ClientBuilder::event_capacity`] for what happens when it is not.
///
/// Ends after [`Event::Closed`].
pub struct Events {
    queue: Arc<queue::Queue>,
}

impl Events {
    pub async fn next(&mut self) -> Option<Event> {
        std::future::poll_fn(|cx| self.queue.poll_next(cx)).await
    }
}

impl futures_util::Stream for Events {
    type Item = Event;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        self.queue.poll_next(cx)
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        self.queue.abandon();
    }
}

impl std::fmt::Debug for Events {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Events").finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum Event {
    /// The complete selected state at one commit: replace everything you
    /// derived from earlier events.
    Snapshot {
        snapshot: SnapshotView,
        warnings: Vec<DiagnosticView>,
        cause: SnapshotCause,
    },
    /// One complete commit, filtered to the selection.
    Update(UpdateView),
    /// The setup batch failed after a (re)connection: the server rejected it
    /// ([`ClientError::Rejected`]) or it is too large to send. The connection
    /// stays up and other writes proceed.
    SetupRejected(ClientError),
    /// The connection was lost or could not be established; the next attempt
    /// starts after `retry_in`. Derived state is stale until the next
    /// [`Event::Snapshot`].
    Disconnected {
        error: Disconnect,
        retry_in: Duration,
    },
    /// The last event: the client will not reconnect.
    Closed(Closed),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotCause {
    /// The hello of a new connection (the first one or a reconnect).
    Connected,
    /// [`Client::select`] changed the selection.
    Selected,
    /// Updates were dropped because [`Events`] fell behind. Event and command
    /// occurrences in those updates are lost.
    Resync,
}

#[derive(Debug, thiserror::Error)]
pub enum Disconnect {
    #[error("connection failed: {0}")]
    Connection(#[source] ConnectionError),
    #[error("no snapshot answered the hello in time")]
    Timeout,
    /// A normal close by the server, such as `server_shutdown`.
    #[error("the server closed the connection")]
    Ended,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Closed {
    /// [`Client::close`], or every [`Client`] was dropped.
    Requested,
    /// A newer managed connection with the same client name took over (close
    /// code 4001). Reconnecting would take the name back, so the client stops.
    Replaced,
    /// The server rejected the hello; retrying would not help.
    Rejected(ErrorView),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Connecting,
    Connected,
    Reconnecting { retry_in: Duration },
    Closed,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("not connected")]
    NotConnected,
    #[error("the server rejected the request: {} ({:?})", .0.message, .0.code)]
    Rejected(ErrorView),
    #[error("no reply arrived in time")]
    Timeout,
    /// The connection dropped before the reply: the write may or may not have
    /// committed.
    #[error("the connection was lost before the reply; the outcome is unknown")]
    Lost,
    #[error("the message exceeds the 1 MiB limit")]
    TooLarge,
    #[error("the client is closed")]
    Closed,
}

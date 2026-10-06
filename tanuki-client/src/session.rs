use crate::connection::encode_message;
use crate::{Codec, Connection, ConnectionError, SelectedView, protocol::*};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::{
    sync::{Semaphore, mpsc, oneshot, watch},
    task::JoinHandle,
    time::{Instant, timeout},
};
use tracing::Instrument;

#[derive(Clone, Debug)]
pub struct SessionOptions {
    pub codec: Codec,
    pub connect_timeout: Duration,
    pub write_timeout: Duration,
    pub pending_requests: std::num::NonZeroUsize,
    pub raw_capacity: std::num::NonZeroUsize,
}
impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            codec: Codec::Json,
            connect_timeout: Duration::from_secs(10),
            write_timeout: Duration::from_secs(10),
            pending_requests: std::num::NonZeroUsize::new(64).unwrap(),
            raw_capacity: std::num::NonZeroUsize::new(64).unwrap(),
        }
    }
}
#[derive(Clone, Debug, thiserror::Error)]
pub enum SessionEnd {
    #[error("session closed")]
    Closed,
    #[error("raw listener lagged; register again for current retained state")]
    Lagged,
    #[error("connection failed: {0}")]
    Connection(#[source] Arc<ConnectionError>),
    #[error("invalid update sequence: {0}")]
    Sequence(#[source] crate::ClientViewError),
    #[error("native SDK task failed: {0}")]
    Task(#[source] Arc<tokio::task::JoinError>),
    #[error("protocol violation: {0}")]
    Protocol(String),
    #[error("uncorrelated server error: {0:?}")]
    Remote(ErrorView),
}
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Payload(#[from] crate::PayloadError),
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    #[error(transparent)]
    Ended(#[from] SessionEnd),
    #[error("connection/hello deadline exceeded")]
    ConnectTimeout,
    #[error("{parameter} exceeds the runtime queue limit")]
    QueueLimit { parameter: &'static str },
    #[error("{parameter} deadline is outside the monotonic clock range")]
    DeadlineRange { parameter: &'static str },
    #[error("write deadline exceeded before sending")]
    UnsentTimeout,
    #[error("pending request limit reached before sending")]
    QueueFull,
    #[error("session is gone before sending")]
    SessionGone,
    #[error("write outcome unknown: {0}")]
    OutcomeUnknown(#[source] SessionEnd),
    #[error("write outcome unknown: reply deadline exceeded")]
    ReplyTimeout,
    #[error("server rejected write: {0:?}")]
    Rejected(ErrorView),
    #[error("invalid reply: {0}")]
    Reply(#[source] serde_json::Error),
    #[error("topic is not in hello selection: {0}")]
    NotSubscribed(TopicPath),
    #[error("topic handle belongs to another session")]
    WrongSession,
    #[error("empty write batch")]
    EmptyBatch,
    #[error(transparent)]
    Path(#[from] PathParseError),
}
#[derive(Clone, Debug, serde::Deserialize)]
pub struct WriteReceipt {
    pub sequence: u64,
    pub warnings: Vec<DiagnosticView>,
}

pub struct Session {
    pub(crate) inner: Arc<Inner>,
    initial: SnapshotView,
    warnings: Vec<DiagnosticView>,
}
pub(crate) struct Inner {
    commands: mpsc::Sender<DriverCommand>,
    stop: watch::Sender<bool>,
    slots: Arc<Semaphore>,
    options: SessionOptions,
    pub(crate) selection: Selection,
    driver: Mutex<Option<JoinHandle<SessionEnd>>>,
    end: watch::Receiver<Option<SessionEnd>>,
    pub(crate) projections: Mutex<Vec<JoinHandle<()>>>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

enum DriverCommand {
    Write {
        frame: tokio_tungstenite::tungstenite::Message,
        id: RequestId,
        reply: oneshot::Sender<Result<WriteReceipt, ClientError>>,
        deadline: Instant,
    },
    Listen(oneshot::Sender<RawListener>),
}
struct Pending {
    reply: oneshot::Sender<Result<WriteReceipt, ClientError>>,
    deadline: Instant,
}
struct Listener {
    data: mpsc::Sender<UpdateView>,
    terminal: watch::Sender<Option<SessionEnd>>,
}

/// Owns the socket driver and all projections. Topic handles only borrow its lifetime.
impl Session {
    pub async fn connect(
        url: &str,
        client: ClientName,
        selectors: Vec<Selector>,
    ) -> Result<Self, ClientError> {
        Self::connect_with_options(url, client, selectors, SessionOptions::default()).await
    }
    pub async fn connect_with_options(
        url: &str,
        client: ClientName,
        selectors: Vec<Selector>,
        options: SessionOptions,
    ) -> Result<Self, ClientError> {
        for (parameter, capacity) in [
            ("pending_requests", options.pending_requests),
            ("raw_capacity", options.raw_capacity),
        ] {
            if capacity.get() > Semaphore::MAX_PERMITS {
                return Err(ClientError::QueueLimit { parameter });
            }
        }
        for (parameter, duration) in [
            ("connect_timeout", options.connect_timeout),
            ("write_timeout", options.write_timeout),
        ] {
            if Instant::now().checked_add(duration).is_none() {
                return Err(ClientError::DeadlineRange { parameter });
            }
        }
        let span = tracing::info_span!("tanuki_session", client = %client);
        let selection = Selection::new(selectors.clone());
        let (connection, initial, warnings) = timeout(options.connect_timeout, async {
            let mut connection = Connection::open_with_codec(url, options.codec).await?;
            let id = RequestId::new("hello".into());
            connection
                .send(&ClientMessage::Hello {
                    request_id: id.clone(),
                    client,
                    selectors,
                })
                .await?;
            match connection.recv().await? {
                Some(ServerMessage::Snapshot {
                    request_id,
                    sequence,
                    nodes,
                    warnings,
                }) if request_id == id => {
                    Ok((connection, SnapshotView { sequence, nodes }, warnings))
                }
                Some(ServerMessage::Error { error, .. }) => Err(ClientError::Rejected(error)),
                _ => Err(ClientError::Ended(SessionEnd::Protocol(
                    "expected correlated hello snapshot".into(),
                ))),
            }
        })
        .await
        .map_err(|_| ClientError::ConnectTimeout)??;
        let (commands, rx) = mpsc::channel(options.pending_requests.get());
        let (stop, stopped) = watch::channel(false);
        let (finished, end) = watch::channel(None);
        let inner = Arc::new(Inner {
            commands,
            stop,
            slots: Arc::new(Semaphore::new(options.pending_requests.get())),
            options: options.clone(),
            selection,
            driver: Mutex::new(None),
            end,
            projections: Mutex::new(vec![]),
        });
        let driver = tokio::spawn(
            run_driver(
                connection,
                rx,
                stopped,
                initial.clone(),
                options.raw_capacity.get(),
                options.pending_requests.get(),
                finished,
            )
            .instrument(span),
        );
        *inner.driver.lock().unwrap() = Some(driver);
        Ok(Self {
            inner,
            initial,
            warnings,
        })
    }
    pub fn termination(&self) -> Option<SessionEnd> {
        self.inner.end.borrow().clone()
    }
    pub async fn closed(&self) -> SessionEnd {
        let mut end = self.inner.end.clone();
        loop {
            if let Some(reason) = end.borrow().clone() {
                return reason;
            }
            if end.changed().await.is_err() {
                return SessionEnd::Closed;
            }
        }
    }
    pub fn selection(&self) -> &Selection {
        &self.inner.selection
    }
    pub fn initial_snapshot(&self) -> &SnapshotView {
        &self.initial
    }
    pub fn hello_warnings(&self) -> &[DiagnosticView] {
        &self.warnings
    }
    pub async fn write(&self, operations: Vec<WireOperation>) -> Result<WriteReceipt, ClientError> {
        write(&Arc::downgrade(&self.inner), operations).await
    }
    pub async fn listen_updates(&self) -> Result<RawListener, ClientError> {
        let (tx, rx) = oneshot::channel();
        self.inner
            .commands
            .send(DriverCommand::Listen(tx))
            .await
            .map_err(|_| ClientError::SessionGone)?;
        rx.await.map_err(|_| ClientError::SessionGone)
    }
    pub async fn close(self) -> Result<(), ClientError> {
        self.inner.stop.send_replace(true);
        let driver = self.inner.driver.lock().unwrap().take();
        let reason = if let Some(driver) = driver {
            driver.await.map_err(|e| SessionEnd::Task(Arc::new(e)))?
        } else {
            SessionEnd::Closed
        };
        let projections = std::mem::take(&mut *self.inner.projections.lock().unwrap());
        for task in projections {
            if let Err(e) = task.await
                && !e.is_cancelled()
            {
                return Err(SessionEnd::Task(Arc::new(e)).into());
            }
        }
        if matches!(reason, SessionEnd::Closed) {
            Ok(())
        } else {
            Err(reason.into())
        }
    }
}

pub(crate) async fn write(
    inner: &Weak<Inner>,
    operations: Vec<WireOperation>,
) -> Result<WriteReceipt, ClientError> {
    if operations.is_empty() {
        return Err(ClientError::EmptyBatch);
    }
    let inner = inner.upgrade().ok_or(ClientError::SessionGone)?;
    let _permit = inner
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| ClientError::QueueFull)?;
    // IDs are unique within this session even after cancellation. No reuse on late replies.
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = RequestId::new(
        NEXT_ID
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_string(),
    );
    let frame = encode_message(
        &ClientMessage::Write {
            request_id: id.clone(),
            operations,
        },
        inner.options.codec,
    )?;
    let deadline = Instant::now()
        .checked_add(inner.options.write_timeout)
        .ok_or(ClientError::DeadlineRange {
            parameter: "write_timeout",
        })?;
    let commands = inner.commands.clone();
    drop(inner);
    let (reply, rx) = oneshot::channel();
    commands
        .try_send(DriverCommand::Write {
            frame,
            id,
            reply,
            deadline,
        })
        .map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => ClientError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => ClientError::SessionGone,
        })?;
    // Driver owns the deadline: queued unsent writes are checked before transmission.
    rx.await
        .map_err(|_| ClientError::OutcomeUnknown(SessionEnd::Closed))?
}

/// A retained baseline followed by complete deltas. Lag is terminal and reported once.
pub struct RawListener {
    initial: SnapshotView,
    data: mpsc::Receiver<UpdateView>,
    terminal: watch::Receiver<Option<SessionEnd>>,
    ended: bool,
}
#[cfg(test)]
pub(crate) fn raw_channel(
    initial: SnapshotView,
    capacity: usize,
) -> (
    RawListener,
    mpsc::Sender<UpdateView>,
    watch::Sender<Option<SessionEnd>>,
) {
    let (data, rx) = mpsc::channel(capacity);
    let (terminal, end) = watch::channel(None);
    (
        RawListener {
            initial,
            data: rx,
            terminal: end,
            ended: false,
        },
        data,
        terminal,
    )
}
impl RawListener {
    pub fn into_stream(self) -> impl futures_util::Stream<Item = Result<UpdateView, SessionEnd>> {
        futures_util::stream::unfold(self, |mut listener| async move {
            match listener.recv().await {
                Ok(Some(update)) => Some((Ok(update), listener)),
                Err(error) => Some((Err(error), listener)),
                Ok(None) => None,
            }
        })
    }
    pub fn initial_snapshot(&self) -> &SnapshotView {
        &self.initial
    }
    pub async fn recv(&mut self) -> Result<Option<UpdateView>, SessionEnd> {
        if self.ended {
            return Ok(None);
        }
        loop {
            if let Some(reason) = self.terminal.borrow().clone() {
                if matches!(reason, SessionEnd::Closed)
                    && let Ok(update) = self.data.try_recv()
                {
                    return Ok(Some(update));
                }
                self.ended = true;
                return if matches!(reason, SessionEnd::Closed) {
                    Ok(None)
                } else {
                    Err(reason)
                };
            }
            tokio::select! { biased;
                result = self.terminal.changed() => { if result.is_err() { self.ended = true; return Ok(None); } }
                update = self.data.recv() => { if update.is_none() { self.ended = true; } return Ok(update); }
            }
        }
    }
}

async fn run_driver(
    connection: Connection,
    mut commands: mpsc::Receiver<DriverCommand>,
    mut stopped: watch::Receiver<bool>,
    initial: SnapshotView,
    raw_capacity: usize,
    outgoing_capacity: usize,
    finished: watch::Sender<Option<SessionEnd>>,
) -> SessionEnd {
    let (mut sink, mut stream) = connection.socket.split();
    // Socket writes can wait on network backpressure; they never block reading or registration.
    let (outgoing, mut sends) = mpsc::channel(outgoing_capacity);
    let (failed, mut failures) = mpsc::channel(1);
    let mut writer = tokio::spawn(async move {
        while let Some(frame) = sends.recv().await {
            if let Err(source) = sink.send(frame).await {
                let _ = failed.send(source).await;
                break;
            }
        }
        let _ = sink.close().await;
    });
    let mut view = SelectedView::from_snapshot(initial);
    let mut pending: HashMap<RequestId, Pending> = HashMap::new();
    let mut listeners: Vec<Listener> = Vec::new();
    let reason = loop {
        // In particular, honor a Session dropped before this task first runs.
        // A ready command must not win the select and start a new write then.
        if *stopped.borrow() {
            break SessionEnd::Closed;
        }
        pending.retain(|_, waiter| !waiter.reply.is_closed());
        listeners.retain(|listener| !listener.data.is_closed());
        let next_deadline = pending
            .values()
            .map(|p| p.deadline)
            .min()
            .unwrap_or(Instant::now() + Duration::from_secs(3600));
        tokio::select! {
            _ = stopped.changed() => break SessionEnd::Closed,
            Some(error) = failures.recv() => break SessionEnd::Connection(Arc::new(ConnectionError::Transport(error))),
            _ = tokio::time::sleep_until(next_deadline) => {
                let now = Instant::now();
                let expired: Vec<_> = pending.iter().filter(|(_, p)| p.deadline <= now).map(|(id, _)| id.clone()).collect();
                for id in expired { if let Some(p) = pending.remove(&id) { let _ = p.reply.send(Err(ClientError::ReplyTimeout)); } }
            }
            command = commands.recv() => match command {
                None => break SessionEnd::Closed,
                Some(DriverCommand::Listen(reply)) => {
                    if reply.is_closed() { continue; }
                    let (data, rx) = mpsc::channel(raw_capacity);
                    let (terminal, end) = watch::channel(None);
                    let listener = RawListener { initial: SnapshotView { sequence: view.sequence(), nodes: view.nodes().clone() }, data: rx, terminal: end, ended: false };
                    if reply.send(listener).is_ok() { listeners.push(Listener { data, terminal }); }
                }
                Some(DriverCommand::Write { frame, id, reply, deadline }) => {
                    if reply.is_closed() { continue; }
                    if deadline <= Instant::now() { let _ = reply.send(Err(ClientError::UnsentTimeout)); continue; }
                    match outgoing.try_send(frame) {
                        Ok(()) => { pending.insert(id, Pending { reply, deadline }); }
                        Err(_) => { let _ = reply.send(Err(ClientError::QueueFull)); }
                    }
                }
            },
            frame = stream.next() => {
                use tokio_tungstenite::tungstenite::Message;
                let message = match frame {
                    Some(Ok(Message::Text(text))) => serde_json::from_str::<ServerMessage>(&text).map_err(ConnectionError::from),
                    Some(Ok(Message::Binary(bytes))) => decode_messagepack::<ServerMessage>(&bytes).map_err(ConnectionError::from),
                    Some(Ok(Message::Ping(bytes))) => {
                        if outgoing.try_send(Message::Pong(bytes)).is_err() { break SessionEnd::Protocol("writer queue full responding to ping".into()); }
                        continue;
                    }
                    Some(Ok(Message::Pong(_) | Message::Frame(_))) => continue,
                    Some(Ok(Message::Close(close))) => {
                        break match close { Some(close) if u16::from(close.code) != 1000 && u16::from(close.code) != 1001 => SessionEnd::Connection(Arc::new(ConnectionError::Closed { code: close.code.into(), reason: close.reason.to_string() })), _ => SessionEnd::Closed };
                    }
                    Some(Err(source)) => break SessionEnd::Connection(Arc::new(ConnectionError::Transport(source))),
                    None => break SessionEnd::Closed,
                };
                let message = match message { Ok(message) => message, Err(error) => break SessionEnd::Connection(Arc::new(error)) };
                match message {
                    ServerMessage::Update { sequence, changes } => {
                        let update = UpdateView { sequence, changes };
                        if let Err(error) = view.apply(update.clone()) { break SessionEnd::Sequence(error); }
                        listeners.retain(|listener| match listener.data.try_send(update.clone()) {
                            Ok(()) => true,
                            Err(mpsc::error::TrySendError::Full(_)) => { tracing::warn!(sequence = update.sequence, capacity = raw_capacity, "raw listener ended after queue overflow"); listener.terminal.send_replace(Some(SessionEnd::Lagged)); false }
                            Err(mpsc::error::TrySendError::Closed(_)) => false,
                        });
                    }
                    ServerMessage::Reply { request_id, result } => {
                        if let Some(waiter) = pending.remove(&request_id) { let _ = waiter.reply.send(serde_json::from_value(result).map_err(ClientError::Reply)); }
                        else { tracing::debug!(request_id = %request_id.as_str(), "ignored late reply"); }
                    }
                    ServerMessage::Error { request_id: Some(id), error } => {
                        if let Some(waiter) = pending.remove(&id) { let _ = waiter.reply.send(Err(ClientError::Rejected(error))); }
                        else { tracing::debug!(request_id = %id.as_str(), ?error, "ignored late rejection"); }
                    }
                    ServerMessage::Error { request_id: None, error } => break SessionEnd::Remote(error),
                    ServerMessage::Snapshot { .. } => break SessionEnd::Protocol("second snapshot after hello".into()),
                }
            }
        }
    };
    if !matches!(reason, SessionEnd::Closed) {
        tracing::warn!(?reason, "Tanuki session terminated abnormally");
    }
    // Commands still here were never handed to the writer. Reject them before
    // publishing termination; subsequent writes also fail locally immediately.
    commands.close();
    while let Ok(command) = commands.try_recv() {
        if let DriverCommand::Write { reply, .. } = command {
            let _ = reply.send(Err(ClientError::SessionGone));
        }
    }
    finished.send_replace(Some(reason.clone()));
    for listener in listeners {
        listener.terminal.send_replace(Some(reason.clone()));
    }
    for (_, waiter) in pending {
        let _ = waiter
            .reply
            .send(Err(ClientError::OutcomeUnknown(reason.clone())));
    }
    // Best-effort close must not hang shutdown behind a blocked socket writer.
    drop(outgoing);
    if timeout(Duration::from_secs(1), &mut writer).await.is_err() {
        writer.abort();
        let _ = writer.await;
    }
    reason
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn clean_closure_drains_already_admitted_batches() {
        let (mut raw, data, terminal) = raw_channel(
            SnapshotView {
                sequence: 0,
                nodes: Default::default(),
            },
            1,
        );
        data.try_send(UpdateView {
            sequence: 1,
            changes: vec![],
        })
        .unwrap();
        terminal.send_replace(Some(SessionEnd::Closed));
        drop(data);
        assert_eq!(raw.recv().await.unwrap().unwrap().sequence, 1);
        assert!(raw.recv().await.unwrap().is_none());
    }
}

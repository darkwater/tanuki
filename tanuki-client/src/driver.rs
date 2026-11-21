//! The session's background task: one connection at a time, reconnecting
//! with backoff. It owns the selected-state cache and every registered
//! observer, listener and controller; API calls reach it as [`Command`]s.
//!
//! Per connection: hello with the union of registered selectors, re-claim
//! controlled inputs, publish fresh baselines, then serve commands and frames.
//! Because the server sends a write's own update before its reply, the cache
//! (and every observer) already reflects a write when its receipt resolves.
use crate::{
    Connection, ConnectionError, Event, Resync, Snapshot,
    connection::{Incoming, Message, decode_frame, encode_message},
    listen::Queue,
    protocol::*,
    runtime::{self, Instant},
    session::{ClientError, Closed, Disconnect, Reconnect, SessionBuilder, Status, WriteReceipt},
};
use futures_util::{SinkExt, StreamExt};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{mpsc, oneshot, watch};

pub(crate) enum Command {
    Write {
        operations: Vec<WireOperation>,
        reply: oneshot::Sender<Result<WriteReceipt, ClientError>>,
        deadline: Instant,
    },
    Observe {
        id: u64,
        selectors: Arc<[Selector]>,
        sender: watch::Sender<Option<Snapshot>>,
    },
    Listen {
        id: u64,
        selectors: Arc<[Selector]>,
        queue: Arc<Queue>,
    },
    Control {
        id: u64,
        topic: TopicPath,
        input: WireInputKind,
        release: WireRelease,
        claim: watch::Sender<ClaimState>,
        reply: oneshot::Sender<Result<(), ClientError>>,
    },
    Unregister(u64),
    Close,
}

/// A controller's claim as the driver knows it.
#[derive(Clone, Debug)]
pub(crate) enum ClaimState {
    /// Not claimed on the current connection (yet).
    Pending,
    /// Claim `id`, established by the commit `sequence`.
    Held {
        id: u64,
        sequence: u64,
    },
    Failed(ErrorView),
}

struct ObserverEntry {
    id: u64,
    selectors: Arc<[Selector]>,
    sender: watch::Sender<Option<Snapshot>>,
    ready: bool,
}
struct ListenerEntry {
    id: u64,
    selectors: Arc<[Selector]>,
    queue: Arc<Queue>,
    ready: bool,
    started: bool,
}
struct ControllerEntry {
    id: u64,
    topic: TopicPath,
    input: WireInputKind,
    release: WireRelease,
    claim: watch::Sender<ClaimState>,
}

enum Pending {
    Write {
        reply: oneshot::Sender<Result<WriteReceipt, ClientError>>,
        deadline: Instant,
    },
    Claim {
        controller: u64,
        reply: Option<oneshot::Sender<Result<(), ClientError>>>,
        deadline: Instant,
        /// Defining an input someone else claimed is rejected, while claiming
        /// alone takes it over. An explicit `control()` retries claim-only.
        take_over: bool,
    },
    Select {
        selection: Vec<Selector>,
    },
}

impl Pending {
    fn deadline(&self) -> Option<Instant> {
        match self {
            Self::Write { deadline, .. } | Self::Claim { deadline, .. } => Some(*deadline),
            Self::Select { .. } => None,
        }
    }
}

enum End {
    Close,
    Replaced,
    Lost(Disconnect),
}

struct Driver {
    options: SessionBuilder,
    client: ClientName,
    commands: mpsc::UnboundedReceiver<Command>,
    status: watch::Sender<Status>,
    view: SelectedView,
    /// The selection the server holds for the current connection.
    selection: Vec<Selector>,
    observers: Vec<ObserverEntry>,
    listeners: Vec<ListenerEntry>,
    controllers: Vec<ControllerEntry>,
    pending: HashMap<RequestId, Pending>,
    next_request: u64,
}

pub(crate) async fn run(
    options: SessionBuilder,
    client: ClientName,
    fail_first: bool,
    commands: mpsc::UnboundedReceiver<Command>,
    status: watch::Sender<Status>,
) {
    let span = tracing::info_span!("tanuki_session", client = %client);
    let mut driver = Driver {
        options,
        client,
        commands,
        status,
        view: SelectedView::from_snapshot(SnapshotView {
            sequence: 0,
            nodes: Default::default(),
        }),
        selection: Vec::new(),
        observers: Vec::new(),
        listeners: Vec::new(),
        controllers: Vec::new(),
        pending: HashMap::new(),
        next_request: 0,
    };
    let closed = tracing::Instrument::instrument(driver.run(fail_first), span).await;
    driver.finish(closed);
}

impl Driver {
    async fn run(&mut self, fail_first: bool) -> Closed {
        let mut attempt = 0_u32;
        let mut connected_once = false;
        loop {
            let cause = match runtime::timeout(self.options.connect_timeout, self.connect()).await {
                Err(_) => Disconnect::Timeout,
                Ok(Err(cause)) => cause,
                Ok(Ok(connection)) => {
                    attempt = 0;
                    connected_once = true;
                    self.status.send_replace(Status::Connected);
                    tracing::info!("connected to Tanuki");
                    match self.serve(connection).await {
                        End::Close => return Closed::ByClient,
                        End::Replaced => {
                            tracing::warn!("session replaced by another managed connection");
                            return Closed::Replaced;
                        }
                        End::Lost(cause) => cause,
                    }
                }
            };
            for controller in &self.controllers {
                controller.claim.send_replace(ClaimState::Pending);
            }
            let Reconnect::Backoff { initial, max } = self.options.reconnect else {
                return Closed::Failed(cause);
            };
            if fail_first && !connected_once {
                return Closed::Failed(cause);
            }
            let retry_in = initial
                .saturating_mul(2_u32.saturating_pow(attempt.min(16)))
                .min(max);
            attempt = attempt.saturating_add(1);
            tracing::warn!(%cause, attempt, ?retry_in, "Tanuki connection lost; reconnecting");
            self.status.send_replace(Status::Reconnecting {
                attempt,
                retry_in,
                cause,
            });
            if !self.wait(retry_in).await {
                return Closed::ByClient;
            }
        }
    }

    /// Waits before reconnecting while still serving commands. `false` when
    /// the session should close.
    async fn wait(&mut self, delay: Duration) -> bool {
        let deadline = Instant::now() + delay;
        loop {
            tokio::select! {
                () = runtime::sleep_until(deadline) => return true,
                command = self.commands.recv() => match command {
                    None => return false,
                    Some(command) => if !self.handle(command, None) { return false; },
                },
            }
        }
    }

    fn request_id(&mut self) -> RequestId {
        self.next_request += 1;
        RequestId::new(self.next_request.to_string())
    }

    /// Every registered selector, deduplicated.
    fn union(&self) -> Vec<Selector> {
        let mut union: Vec<Selector> = Vec::new();
        let selectors = self
            .observers
            .iter()
            .flat_map(|entry| entry.selectors.iter().cloned())
            .chain(
                self.listeners
                    .iter()
                    .flat_map(|entry| entry.selectors.iter().cloned()),
            )
            .chain(
                self.controllers
                    .iter()
                    .map(|entry| Selector::from(&entry.topic)),
            );
        for selector in selectors {
            if !union.contains(&selector) {
                union.push(selector);
            }
        }
        union
    }

    /// Opens a socket, says hello, re-claims inputs and publishes baselines.
    async fn connect(&mut self) -> Result<Connection, Disconnect> {
        let lost = |error: ConnectionError| Disconnect::Connection(Arc::new(error));
        let mut connection = Connection::open_with_codec(&self.options.url, self.options.codec)
            .await
            .map_err(lost)?;
        let hello = self.request_id();
        self.selection = self.union();
        connection
            .send(&ClientMessage::Hello {
                request_id: hello.clone(),
                client: self.client.clone(),
                selectors: self.selection.clone(),
                session: self.options.mode,
            })
            .await
            .map_err(lost)?;
        match connection.recv().await.map_err(lost)? {
            Some(ServerMessage::Snapshot {
                request_id,
                sequence,
                nodes,
                warnings,
            }) if request_id == hello => {
                for warning in warnings {
                    tracing::warn!(?warning, "Tanuki hello warning");
                }
                self.view = SelectedView::from_snapshot(SnapshotView { sequence, nodes });
            }
            Some(ServerMessage::Error { error, .. }) => return Err(Disconnect::Rejected(error)),
            Some(_) => return Err(protocol("expected the hello snapshot")),
            None => return Err(protocol("connection closed during hello")),
        }
        // Claims belong to managed sessions, which end on disconnect.
        for index in 0..self.controllers.len() {
            self.reclaim(&mut connection, index).await?;
        }
        for index in 0..self.observers.len() {
            self.observers[index].ready = true;
            self.publish_observer(index);
        }
        for index in 0..self.listeners.len() {
            let entry = &mut self.listeners[index];
            let reason = if entry.started {
                Resync::Reconnected
            } else {
                Resync::Started
            };
            entry.ready = true;
            entry.started = true;
            self.reset_listener(index, reason);
        }
        Ok(connection)
    }

    async fn reclaim(
        &mut self,
        connection: &mut Connection,
        index: usize,
    ) -> Result<(), Disconnect> {
        let lost = |error: ConnectionError| Disconnect::Connection(Arc::new(error));
        let request = self.request_id();
        let entry = &self.controllers[index];
        connection
            .send(&ClientMessage::Write {
                request_id: request.clone(),
                // Never take over on reconnect: if another session claimed the
                // input meanwhile, this controller stops instead of fighting.
                operations: claim_operations(entry, true),
            })
            .await
            .map_err(lost)?;
        loop {
            match connection.recv().await.map_err(lost)? {
                Some(ServerMessage::Update { sequence, changes }) => {
                    self.view
                        .apply(UpdateView { sequence, changes })
                        .map_err(|error| protocol(&error.to_string()))?;
                }
                Some(ServerMessage::Reply { request_id, result }) if request_id == request => {
                    let receipt: WriteReceipt = serde_json::from_value(result)
                        .map_err(|error| protocol(&format!("invalid claim reply: {error}")))?;
                    self.claim_committed(index, receipt.sequence);
                    return Ok(());
                }
                Some(ServerMessage::Error {
                    request_id: Some(request_id),
                    error,
                }) if request_id == request => {
                    let entry = &self.controllers[index];
                    tracing::warn!(topic = %entry.topic, ?error, "re-claiming an input was rejected");
                    entry.claim.send_replace(ClaimState::Failed(error));
                    return Ok(());
                }
                Some(_) => return Err(protocol("unexpected message while re-claiming")),
                None => return Err(protocol("connection closed while re-claiming")),
            }
        }
    }

    /// Records the claim made by commit `sequence`, read from the cache that
    /// already holds the commit's update.
    fn claim_committed(&self, index: usize, sequence: u64) {
        let entry = &self.controllers[index];
        let state = match self
            .view
            .nodes()
            .get(&entry.topic)
            .and_then(NodeView::claim)
        {
            Some(claim) => ClaimState::Held {
                id: claim.id,
                sequence,
            },
            None => {
                tracing::warn!(topic = %entry.topic, "claim commit is missing from the cache");
                ClaimState::Pending
            }
        };
        entry.claim.send_replace(state);
    }

    async fn serve(&mut self, connection: Connection) -> End {
        let (mut sink, mut stream) = connection.socket.split();
        let (outgoing, mut sends) = mpsc::channel::<Message>(self.options.max_pending_writes);
        let (failed, mut failures) = mpsc::channel(1);
        let mut writer = runtime::spawn(async move {
            while let Some(frame) = sends.recv().await {
                if let Err(error) = sink.send(frame).await {
                    let _ = failed.send(ConnectionError::from(error)).await;
                    break;
                }
            }
            let _ = sink.close().await;
        });
        let end = loop {
            let deadline = self.pending.values().filter_map(Pending::deadline).min();
            tokio::select! {
                command = self.commands.recv() => match command {
                    None => break End::Close,
                    Some(command) => if !self.handle(command, Some(&outgoing)) { break End::Close; },
                },
                frame = stream.next() => match decode_frame(frame) {
                    Ok(Incoming::Message(message)) => if let Some(end) = self.receive(message, &outgoing) { break end; },
                    Ok(Incoming::Control) => {}
                    Ok(Incoming::Closed) => break End::Lost(protocol("server closed the connection")),
                    Err(ConnectionError::Closed { code: 4001, .. }) => break End::Replaced,
                    Err(error) => break End::Lost(Disconnect::Connection(Arc::new(error))),
                },
                Some(error) = failures.recv() => break End::Lost(Disconnect::Connection(Arc::new(error))),
                () = sleep_until_or_never(deadline) => self.expire(),
            }
        };
        for (_, pending) in self.pending.drain() {
            match pending {
                Pending::Write { reply, .. } => {
                    let _ = reply.send(Err(match end {
                        End::Close => ClientError::Closed(Closed::ByClient),
                        _ => ClientError::OutcomeUnknown,
                    }));
                }
                // Re-claimed on the next connection.
                Pending::Claim { reply, .. } => {
                    if let Some(reply) = reply {
                        let _ = reply.send(Ok(()));
                    }
                }
                Pending::Select { .. } => {}
            }
        }
        // Dropping the queue lets the writer send a close frame; never wait long.
        drop(outgoing);
        if runtime::timeout(Duration::from_secs(1), &mut writer)
            .await
            .is_err()
        {
            writer.abort();
        }
        end
    }

    fn expire(&mut self) {
        let now = Instant::now();
        let expired: Vec<RequestId> = self
            .pending
            .iter()
            .filter(|(_, pending)| pending.deadline().is_some_and(|deadline| deadline <= now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            match self.pending.remove(&id) {
                Some(Pending::Write { reply, .. }) => {
                    let _ = reply.send(Err(ClientError::ReplyTimeout));
                }
                Some(Pending::Claim { reply, .. }) => {
                    if let Some(reply) = reply {
                        let _ = reply.send(Err(ClientError::ReplyTimeout));
                    }
                }
                Some(Pending::Select { .. }) | None => {}
            }
        }
    }

    /// Handles one API command. `link` is the connection's outgoing queue, or
    /// `None` while disconnected. Returns `false` to close the session.
    fn handle(&mut self, command: Command, link: Option<&mpsc::Sender<Message>>) -> bool {
        match command {
            Command::Close => return false,
            Command::Unregister(id) => {
                self.observers.retain(|entry| entry.id != id);
                self.listeners.retain(|entry| entry.id != id);
                self.controllers.retain(|entry| entry.id != id);
            }
            Command::Write {
                operations,
                reply,
                deadline,
            } => {
                let Some(link) = link else {
                    let _ = reply.send(Err(ClientError::Disconnected));
                    return true;
                };
                let request_id = self.request_id();
                let message = ClientMessage::Write {
                    request_id: request_id.clone(),
                    operations,
                };
                match encode_message(&message, self.options.codec) {
                    Err(error) => {
                        let _ = reply.send(Err(ClientError::Encode(error)));
                    }
                    Ok(frame) => match link.try_send(frame) {
                        Ok(()) => {
                            self.pending
                                .insert(request_id, Pending::Write { reply, deadline });
                        }
                        Err(_) => {
                            let _ = reply.send(Err(ClientError::Busy));
                        }
                    },
                }
            }
            Command::Observe {
                id,
                selectors,
                sender,
            } => {
                let ready = self.cover(&selectors, link);
                self.observers.push(ObserverEntry {
                    id,
                    selectors,
                    sender,
                    ready,
                });
                if ready {
                    self.publish_observer(self.observers.len() - 1);
                }
            }
            Command::Listen {
                id,
                selectors,
                queue,
            } => {
                let ready = self.cover(&selectors, link);
                self.listeners.push(ListenerEntry {
                    id,
                    selectors,
                    queue,
                    ready,
                    started: ready,
                });
                if ready {
                    self.reset_listener(self.listeners.len() - 1, Resync::Started);
                }
            }
            Command::Control {
                id,
                topic,
                input,
                release,
                claim,
                reply,
            } => {
                let entry = ControllerEntry {
                    id,
                    topic,
                    input,
                    release,
                    claim,
                };
                let Some(link) = link else {
                    // Claimed when the next connection opens.
                    self.controllers.push(entry);
                    let _ = reply.send(Ok(()));
                    return true;
                };
                let operations = claim_operations(&entry, true);
                self.controllers.push(entry);
                self.send_claim(link, id, operations, reply, true);
            }
        }
        true
    }

    fn send_claim(
        &mut self,
        link: &mpsc::Sender<Message>,
        controller: u64,
        operations: Vec<WireOperation>,
        reply: oneshot::Sender<Result<(), ClientError>>,
        take_over: bool,
    ) {
        let request_id = self.request_id();
        let message = ClientMessage::Write {
            request_id: request_id.clone(),
            operations,
        };
        let sent = encode_message(&message, self.options.codec)
            .map_err(ClientError::Encode)
            .and_then(|frame| link.try_send(frame).map_err(|_| ClientError::Busy));
        match sent {
            Ok(()) => {
                self.pending.insert(
                    request_id,
                    Pending::Claim {
                        controller,
                        reply: Some(reply),
                        deadline: Instant::now() + self.options.write_timeout,
                        take_over,
                    },
                );
            }
            Err(error) => {
                self.controllers.retain(|entry| entry.id != controller);
                let _ = reply.send(Err(error));
            }
        }
    }

    /// Whether `selectors` are already selected on the live connection. If not
    /// and connected, widens the selection; entries become ready when the
    /// answering snapshot arrives (or at the next hello).
    fn cover(&mut self, selectors: &[Selector], link: Option<&mpsc::Sender<Message>>) -> bool {
        let Some(link) = link else {
            return false;
        };
        if selectors
            .iter()
            .all(|selector| self.selection.contains(selector))
        {
            return true;
        }
        let mut selection = self.union();
        for selector in selectors {
            if !selection.contains(selector) {
                selection.push(selector.clone());
            }
        }
        let request_id = self.request_id();
        let message = ClientMessage::Select {
            request_id: request_id.clone(),
            selectors: selection.clone(),
        };
        match encode_message(&message, self.options.codec).map(|frame| link.try_send(frame)) {
            Ok(Ok(())) => {
                self.selection = selection.clone();
                self.pending
                    .insert(request_id, Pending::Select { selection });
            }
            // Picked up by the next hello; the queue is full only when the
            // connection is already failing.
            _ => tracing::warn!("could not send a selection change"),
        }
        false
    }

    /// Handles one server message; `Some` ends the connection.
    fn receive(&mut self, message: ServerMessage, link: &mpsc::Sender<Message>) -> Option<End> {
        match message {
            ServerMessage::Update { sequence, changes } => {
                let update = UpdateView { sequence, changes };
                if let Err(error) = self.view.apply(update.clone()) {
                    return Some(End::Lost(protocol(&error.to_string())));
                }
                self.publish_update(&update);
            }
            ServerMessage::Snapshot {
                request_id,
                sequence,
                nodes,
                ..
            } => {
                let Some(Pending::Select { selection }) = self.pending.remove(&request_id) else {
                    return Some(End::Lost(protocol("unexpected snapshot")));
                };
                self.view = SelectedView::from_snapshot(SnapshotView { sequence, nodes });
                let covered =
                    |selectors: &[Selector]| selectors.iter().all(|s| selection.contains(s));
                for index in 0..self.observers.len() {
                    let entry = &mut self.observers[index];
                    if !entry.ready && covered(&entry.selectors) {
                        entry.ready = true;
                        self.publish_observer(index);
                    }
                }
                for index in 0..self.listeners.len() {
                    let entry = &mut self.listeners[index];
                    if !entry.ready && covered(&entry.selectors) {
                        let reason = if entry.started {
                            Resync::Reconnected
                        } else {
                            Resync::Started
                        };
                        entry.ready = true;
                        entry.started = true;
                        self.reset_listener(index, reason);
                    }
                }
            }
            ServerMessage::Reply { request_id, result } => match self.pending.remove(&request_id) {
                Some(Pending::Write { reply, .. }) => {
                    let _ = reply.send(serde_json::from_value(result).map_err(ClientError::Reply));
                }
                Some(Pending::Claim {
                    controller, reply, ..
                }) => {
                    let receipt = serde_json::from_value::<WriteReceipt>(result);
                    if let Some(index) = self.controllers.iter().position(|e| e.id == controller)
                        && let Ok(receipt) = &receipt
                    {
                        self.claim_committed(index, receipt.sequence);
                    }
                    if let Some(reply) = reply {
                        let _ = reply.send(receipt.map(|_| ()).map_err(ClientError::Reply));
                    }
                }
                Some(Pending::Select { .. }) => {
                    return Some(End::Lost(protocol("select answered with a reply")));
                }
                None => tracing::debug!(request_id = request_id.as_str(), "ignored late reply"),
            },
            ServerMessage::Error {
                request_id: Some(request_id),
                error,
            } => match self.pending.remove(&request_id) {
                Some(Pending::Write { reply, .. }) => {
                    let _ = reply.send(Err(ClientError::Rejected(error)));
                }
                Some(Pending::Claim {
                    controller,
                    reply: Some(reply),
                    take_over: true,
                    ..
                }) if error.code == ErrorCode::ClaimAuthorityRequired => {
                    if let Some(entry) = self.controllers.iter().find(|e| e.id == controller) {
                        tracing::info!(topic = %entry.topic, "taking over an input claimed by another session");
                        let operations = claim_operations(entry, false);
                        self.send_claim(link, controller, operations, reply, false);
                    }
                }
                Some(Pending::Claim {
                    controller, reply, ..
                }) => {
                    self.controllers.retain(|entry| entry.id != controller);
                    if let Some(reply) = reply {
                        let _ = reply.send(Err(ClientError::Rejected(error)));
                    }
                }
                Some(Pending::Select { .. }) => {
                    return Some(End::Lost(Disconnect::Rejected(error)));
                }
                None => tracing::debug!(
                    request_id = request_id.as_str(),
                    ?error,
                    "ignored late error"
                ),
            },
            ServerMessage::Error {
                request_id: None,
                error,
            } => return Some(End::Lost(Disconnect::Rejected(error))),
        }
        None
    }

    fn snapshot(&self, selectors: &Arc<[Selector]>) -> Snapshot {
        Snapshot::project(self.view.sequence(), self.view.nodes(), selectors)
    }

    fn publish_observer(&self, index: usize) {
        let entry = &self.observers[index];
        entry
            .sender
            .send_replace(Some(self.snapshot(&entry.selectors)));
    }

    fn reset_listener(&self, index: usize, reason: Resync) {
        let entry = &self.listeners[index];
        entry.queue.reset(Event::Snapshot {
            snapshot: self.snapshot(&entry.selectors),
            reason,
        });
    }

    fn publish_update(&self, update: &UpdateView) {
        let matches = |selectors: &[Selector], change: &ChangeView| {
            selectors
                .iter()
                .any(|selector| selector.matches(change.topic()))
        };
        for (index, entry) in self.observers.iter().enumerate() {
            let retained = update.changes.iter().any(|change| {
                matches!(
                    change,
                    ChangeView::Upsert { .. } | ChangeView::Removed { .. }
                ) && matches(&entry.selectors, change)
            });
            if entry.ready && retained {
                self.publish_observer(index);
            }
        }
        for (index, entry) in self.listeners.iter().enumerate() {
            if !entry.ready {
                continue;
            }
            let changes: Vec<ChangeView> = update
                .changes
                .iter()
                .filter(|change| matches(&entry.selectors, change))
                .cloned()
                .collect();
            if changes.is_empty() {
                continue;
            }
            let event = Event::Update(UpdateView {
                sequence: update.sequence,
                changes,
            });
            if entry.queue.push(event).is_err() {
                tracing::warn!(listener = entry.id, "listener lagged; resynchronising");
                self.reset_listener(index, Resync::Lagged);
            }
        }
    }

    fn finish(&mut self, closed: Closed) {
        tracing::info!(%closed, "Tanuki session ended");
        self.status.send_replace(Status::Closed(closed.clone()));
        self.commands.close();
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Command::Write { reply, .. } => {
                    let _ = reply.send(Err(ClientError::Closed(closed.clone())));
                }
                Command::Control { reply, .. } => {
                    let _ = reply.send(Err(ClientError::Closed(closed.clone())));
                }
                _ => {}
            }
        }
        for entry in &self.listeners {
            entry.queue.close();
        }
        // Dropping the senders ends observers and controllers.
        self.observers.clear();
        self.listeners.clear();
        self.controllers.clear();
    }
}

/// Defines (a no-op for an existing input) and claims; `define: false`
/// only claims, which takes over another session's claim.
fn claim_operations(entry: &ControllerEntry, define: bool) -> Vec<WireOperation> {
    let claim = WireOperation::ClaimInput {
        topic: entry.topic.clone(),
        release: entry.release.clone(),
    };
    if !define {
        return vec![claim];
    }
    vec![
        WireOperation::DefineInput {
            topic: entry.topic.clone(),
            kind: entry.input.clone(),
        },
        claim,
    ]
}

fn protocol(message: &str) -> Disconnect {
    Disconnect::Protocol(Arc::from(message))
}

async fn sleep_until_or_never(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => runtime::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

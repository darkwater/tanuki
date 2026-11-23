//! The background task: one [`Connection`] at a time, reconnecting with
//! backoff. It owns the request IDs, the pending requests, the selection and
//! the setup batch; [`Client`](super::Client) calls reach it as [`Command`]s
//! and everything it observes leaves through the event [`Queue`].
//!
//! Per connection: hello with the latest selection, wait for its snapshot,
//! send the setup batch, then serve commands and server messages until the
//! connection ends. The server handles one socket's messages in order, so the
//! setup batch commits before any write sent after it without further gating.
use super::{
    ClientBuilder, ClientError, Closed, Disconnect, Event, SnapshotCause, Status,
    queue::{Pushed, Queue},
};
use crate::{
    Connection, ConnectionError, Receiver, Sender,
    protocol::{
        ClientMessage, CommitView, ErrorView, RequestId, Selector, ServerMessage, WireOperation,
    },
    runtime,
};
use std::{
    collections::HashMap,
    hash::{BuildHasher, RandomState},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{Instrument, debug, info, warn};

type WriteReply = oneshot::Sender<Result<CommitView, ClientError>>;

pub(crate) enum Command {
    Write {
        operations: Vec<WireOperation>,
        reply: WriteReply,
    },
    Select {
        selectors: Vec<Selector>,
        reply: oneshot::Sender<Result<(), ClientError>>,
    },
    SetSetup(Vec<WireOperation>),
    Close,
}

enum Pending {
    Write(WriteReply),
    Select {
        reply: oneshot::Sender<Result<(), ClientError>>,
        previous: Vec<Selector>,
    },
    Setup,
    Resync,
}

/// Why a connected period ended.
enum End {
    Lost(Disconnect),
    Stop(Closed),
}

const HELLO: &str = "hello";

struct Driver {
    options: ClientBuilder,
    commands: mpsc::UnboundedReceiver<Command>,
    status: watch::Sender<Status>,
    queue: Arc<Queue>,
    pending: HashMap<RequestId, Pending>,
    next_request: u64,
    /// A resync select is in flight: updates until its snapshot are stale.
    resyncing: bool,
}

pub(crate) async fn run(
    options: ClientBuilder,
    commands: mpsc::UnboundedReceiver<Command>,
    status: watch::Sender<Status>,
    queue: Arc<Queue>,
) {
    let span = tracing::info_span!("tanuki_client", client = %options.name);
    let mut driver = Driver {
        options,
        commands,
        status,
        queue,
        pending: HashMap::new(),
        next_request: 0,
        resyncing: false,
    };
    let closed = driver.run().instrument(span).await;
    info!(?closed, "client stopped");
    driver.status.send_replace(Status::Closed);
    driver.queue.push(Event::Closed(closed));
    driver.queue.finish();
}

impl Driver {
    async fn run(&mut self) -> Closed {
        let mut failures = 0u32;
        loop {
            self.status.send_replace(Status::Connecting);
            let error = match self.connect().await {
                Ok(Some(connection)) => {
                    failures = 0;
                    match self.serve(connection).await {
                        End::Stop(closed) => return closed,
                        End::Lost(error) => error,
                    }
                }
                Ok(None) => return Closed::Requested,
                Err(Attempt::Rejected(error)) => return Closed::Rejected(error),
                Err(Attempt::Failed(error)) => error,
            };
            let retry_in = self.delay(failures);
            failures = failures.saturating_add(1);
            warn!(%error, ?retry_in, "disconnected");
            self.status.send_replace(Status::Reconnecting { retry_in });
            self.queue.push(Event::Disconnected { error, retry_in });
            if !self.wait(retry_in).await {
                return Closed::Requested;
            }
        }
    }

    /// Equal jitter: half the exponential delay plus a random part of the rest.
    fn delay(&self, failures: u32) -> Duration {
        let backoff = self.options.backoff;
        let full = backoff
            .initial
            .saturating_mul(2u32.saturating_pow(failures.min(16)))
            .min(backoff.max);
        let random = RandomState::new().hash_one(failures);
        #[expect(clippy::cast_precision_loss, reason = "a jitter fraction")]
        let fraction = (random >> 11) as f64 / (1u64 << 53) as f64;
        full / 2 + (full / 2).mul_f64(fraction)
    }

    /// Opens a connection and completes the hello, serving commands while
    /// waiting. `Ok(None)` means the client was closed meanwhile.
    async fn connect(&mut self) -> Result<Option<Connection>, Attempt> {
        let hello = ClientMessage::Hello {
            request_id: RequestId::new(HELLO.into()),
            client: self.options.name.clone(),
            selectors: self.options.selectors.clone(),
            session: self.options.session,
        };
        let url = self.options.url.clone();
        let handshake = runtime::timeout(
            self.options.connect_timeout,
            handshake(&url, self.options.codec, hello),
        );
        tokio::pin!(handshake);
        loop {
            tokio::select! {
                result = &mut handshake => {
                    let (connection, snapshot, warnings) = result
                        .map_err(|runtime::Elapsed| Attempt::Failed(Disconnect::Timeout))??;
                    info!(sequence = snapshot.sequence, "connected");
                    self.status.send_replace(Status::Connected);
                    self.queue.push(Event::Snapshot {
                        snapshot,
                        warnings,
                        cause: SnapshotCause::Connected,
                    });
                    return Ok(Some(connection));
                }
                command = self.commands.recv() => {
                    if !self.offline(command) {
                        return Ok(None);
                    }
                }
            }
        }
    }

    /// Serves commands while disconnected for `duration`. False when closed.
    async fn wait(&mut self, duration: Duration) -> bool {
        let sleep = runtime::sleep(duration);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = &mut sleep => return true,
                command = self.commands.recv() => if !self.offline(command) { return false },
            }
        }
    }

    /// Handles a command with no connection. False when the client closes.
    fn offline(&mut self, command: Option<Command>) -> bool {
        match command {
            None | Some(Command::Close) => return false,
            Some(Command::Write { reply, .. }) => {
                let _ = reply.send(Err(ClientError::NotConnected));
            }
            Some(Command::Select { selectors, reply }) => {
                self.options.selectors = selectors;
                let _ = reply.send(Ok(()));
            }
            Some(Command::SetSetup(operations)) => self.options.setup = operations,
        }
        true
    }

    async fn serve(&mut self, connection: Connection) -> End {
        let (sender, mut receiver) = connection.split();
        let end = self.serve_connected(&sender, &mut receiver).await;
        self.resyncing = false;
        for (_, pending) in self.pending.drain() {
            match pending {
                Pending::Write(reply) => {
                    let _ = reply.send(Err(ClientError::Lost));
                }
                // The selection is kept and the next hello uses it.
                Pending::Select { reply, .. } => {
                    let _ = reply.send(Ok(()));
                }
                Pending::Setup | Pending::Resync => {}
            }
        }
        if let End::Stop(Closed::Requested) = end {
            close(&sender, &mut receiver).await;
        }
        end
    }

    async fn serve_connected(&mut self, sender: &Sender, receiver: &mut Receiver) -> End {
        if !self.options.setup.is_empty() {
            let request_id = self.request_id();
            let message = ClientMessage::Write {
                request_id: request_id.clone(),
                operations: self.options.setup.clone(),
            };
            match sender.send(&message).await {
                Ok(()) => {
                    self.pending.insert(request_id, Pending::Setup);
                }
                Err(ConnectionError::MessageTooLarge) => {
                    warn!("setup batch exceeds the message limit");
                    self.queue.push(Event::SetupRejected(ClientError::TooLarge));
                }
                Err(error) => return End::Lost(Disconnect::Connection(error)),
            }
        }
        loop {
            let result = tokio::select! {
                message = receiver.recv() => match message {
                    Ok(Some(message)) => self.receive(message, sender).await,
                    Ok(None) => Err(End::Lost(Disconnect::Ended)),
                    Err(ConnectionError::Closed { code: 4001, reason }) => {
                        warn!(%reason, "replaced by a newer connection with the same name");
                        Err(End::Stop(Closed::Replaced))
                    }
                    Err(error) => Err(End::Lost(Disconnect::Connection(error))),
                },
                command = self.commands.recv() => self.online(command, sender).await,
            };
            if let Err(end) = result {
                return end;
            }
        }
    }

    async fn online(&mut self, command: Option<Command>, sender: &Sender) -> Result<(), End> {
        match command {
            None | Some(Command::Close) => return Err(End::Stop(Closed::Requested)),
            Some(Command::Write { operations, reply }) => {
                self.write(operations, reply, sender).await?;
            }
            Some(Command::Select { selectors, reply }) => {
                let previous = std::mem::replace(&mut self.options.selectors, selectors.clone());
                let request_id = self.request_id();
                self.pending
                    .insert(request_id.clone(), Pending::Select { reply, previous });
                self.send(
                    sender,
                    &ClientMessage::Select {
                        request_id,
                        selectors,
                    },
                )
                .await?;
            }
            Some(Command::SetSetup(operations)) => self.options.setup = operations,
        }
        Ok(())
    }

    async fn write(
        &mut self,
        operations: Vec<WireOperation>,
        reply: WriteReply,
        sender: &Sender,
    ) -> Result<(), End> {
        if reply.is_closed() {
            debug!("dropping a write whose caller gave up");
            return Ok(());
        }
        let request_id = self.request_id();
        let message = ClientMessage::Write {
            request_id: request_id.clone(),
            operations,
        };
        match sender.send(&message).await {
            Ok(()) => {
                self.pending.insert(request_id, Pending::Write(reply));
                Ok(())
            }
            Err(ConnectionError::MessageTooLarge) => {
                let _ = reply.send(Err(ClientError::TooLarge));
                Ok(())
            }
            Err(error) => {
                let _ = reply.send(Err(ClientError::Lost));
                Err(End::Lost(Disconnect::Connection(error)))
            }
        }
    }

    async fn send(&mut self, sender: &Sender, message: &ClientMessage) -> Result<(), End> {
        sender
            .send(message)
            .await
            .map_err(|error| End::Lost(Disconnect::Connection(error)))
    }

    async fn receive(&mut self, message: ServerMessage, sender: &Sender) -> Result<(), End> {
        match message {
            ServerMessage::Update { sequence, changes } => {
                if self.resyncing {
                    debug!(
                        sequence,
                        "dropping an update superseded by the pending resync"
                    );
                } else if self.queue.push(Event::Update(crate::protocol::UpdateView {
                    sequence,
                    changes,
                })) == Pushed::Overflowed
                {
                    warn!(sequence, "event queue full; dropping updates and resyncing");
                    self.resyncing = true;
                    let request_id = self.request_id();
                    self.pending.insert(request_id.clone(), Pending::Resync);
                    let selectors = self.options.selectors.clone();
                    self.send(
                        sender,
                        &ClientMessage::Select {
                            request_id,
                            selectors,
                        },
                    )
                    .await?;
                }
            }
            ServerMessage::Snapshot {
                request_id,
                sequence,
                nodes,
                warnings,
            } => {
                let snapshot = crate::protocol::SnapshotView { sequence, nodes };
                match self.pending.remove(&request_id) {
                    Some(Pending::Select { reply, .. }) => {
                        self.queue.push(Event::Snapshot {
                            snapshot,
                            warnings,
                            cause: SnapshotCause::Selected,
                        });
                        let _ = reply.send(Ok(()));
                    }
                    Some(Pending::Resync) => {
                        self.resyncing = false;
                        self.queue.push(Event::Snapshot {
                            snapshot,
                            warnings,
                            cause: SnapshotCause::Resync,
                        });
                    }
                    other => self.unexpected(&request_id, other, "snapshot"),
                }
            }
            ServerMessage::Reply { request_id, result } => match self.pending.remove(&request_id) {
                Some(Pending::Write(reply)) => {
                    let _ = reply.send(Ok(result));
                }
                Some(Pending::Setup) => {
                    debug!(sequence = result.sequence, "setup committed");
                }
                other => self.unexpected(&request_id, other, "reply"),
            },
            ServerMessage::Error {
                request_id: Some(request_id),
                error,
            } => match self.pending.remove(&request_id) {
                Some(Pending::Write(reply)) => {
                    let _ = reply.send(Err(ClientError::Rejected(error)));
                }
                Some(Pending::Select { reply, previous }) => {
                    warn!(?error, "selection rejected; keeping the previous one");
                    self.options.selectors = previous;
                    let _ = reply.send(Err(ClientError::Rejected(error)));
                }
                Some(Pending::Setup) => {
                    warn!(?error, "setup batch rejected");
                    self.queue
                        .push(Event::SetupRejected(ClientError::Rejected(error)));
                }
                Some(Pending::Resync) => {
                    // Updates were lost and the selection cannot be refreshed:
                    // reconnecting restores a coherent snapshot.
                    warn!(?error, "resync rejected; reconnecting");
                    return Err(End::Lost(Disconnect::Ended));
                }
                None => warn!(?request_id, ?error, "error for an unknown request"),
            },
            ServerMessage::Error {
                request_id: None,
                error,
            } => warn!(?error, "uncorrelated server error"),
        }
        Ok(())
    }

    fn unexpected(&mut self, request_id: &RequestId, pending: Option<Pending>, what: &str) {
        warn!(?request_id, what, "unexpected correlated message");
        if let Some(pending) = pending {
            self.pending.insert(request_id.clone(), pending);
        }
    }

    fn request_id(&mut self) -> RequestId {
        self.next_request += 1;
        RequestId::new(self.next_request.to_string())
    }
}

enum Attempt {
    Failed(Disconnect),
    Rejected(ErrorView),
}

impl From<Disconnect> for Attempt {
    fn from(error: Disconnect) -> Self {
        Self::Failed(error)
    }
}

async fn handshake(
    url: &str,
    codec: crate::Codec,
    hello: ClientMessage,
) -> Result<
    (
        Connection,
        crate::protocol::SnapshotView,
        Vec<crate::protocol::DiagnosticView>,
    ),
    Attempt,
> {
    let mut connection = Connection::open_with_codec(url, codec)
        .await
        .map_err(Disconnect::Connection)?;
    connection
        .send(&hello)
        .await
        .map_err(Disconnect::Connection)?;
    // The server answers a hello with exactly one snapshot before anything else.
    match connection.recv().await.map_err(Disconnect::Connection)? {
        Some(ServerMessage::Snapshot {
            sequence,
            nodes,
            warnings,
            ..
        }) => Ok((
            connection,
            crate::protocol::SnapshotView { sequence, nodes },
            warnings,
        )),
        Some(ServerMessage::Error { error, .. }) => Err(Attempt::Rejected(error)),
        Some(other) => {
            warn!(
                ?other,
                "the server answered hello with something other than a snapshot"
            );
            Err(Disconnect::Ended.into())
        }
        None => Err(Disconnect::Ended.into()),
    }
}

/// A normal close, then reading until the server acknowledges it (bounded).
async fn close(sender: &Sender, receiver: &mut Receiver) {
    if sender.close().await.is_err() {
        return;
    }
    let drain = async { while let Ok(Some(_)) = receiver.recv().await {} };
    if runtime::timeout(Duration::from_secs(1), drain)
        .await
        .is_err()
    {
        debug!("the server did not acknowledge the close in time");
    }
}

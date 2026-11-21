//! Claimed inputs: the controller side of desired values and commands.
use crate::{
    Current, Event, Listener, Observer, Occurrence, Resync, Snapshot, Topic,
    driver::ClaimState,
    protocol::{
        ChangeView, ClaimView, ClientName, ErrorView, FromValue, NodeView, PayloadError, TopicPath,
    },
    session::Shared,
    topic::{Command, Desired, InputKind},
};
use std::{collections::VecDeque, marker::PhantomData, sync::Weak};
use tokio::sync::watch;

/// A change to a controlled desired input.
#[derive(Clone, Debug, PartialEq)]
pub enum Request<T> {
    /// A new request (each submission is reported once, even if its value
    /// equals the previous one).
    Submitted(Current<T>),
    /// The pending request was cleared or expired.
    Cleared,
}

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// Another session claimed the input, or it was removed. The controller
    /// stops; call [`Session::control`](crate::Session::control) to take it back.
    #[error("claim on {topic} was lost{}", by.as_ref().map(|by| format!(" to {by}")).unwrap_or_default())]
    ClaimLost {
        topic: TopicPath,
        by: Option<ClientName>,
    },
    /// The server rejected re-claiming after a reconnect. The controller stops.
    #[error("server rejected the claim on {topic}: {} ({})", error.message, error.code)]
    ClaimRejected { topic: TopicPath, error: ErrorView },
    /// A request or command could not be decoded; the controller continues.
    #[error("could not decode {topic}: {source}")]
    Payload {
        topic: TopicPath,
        #[source]
        source: PayloadError,
    },
    /// Commands may have been lost while disconnected or lagging; the
    /// controller continues.
    #[error("commands may have been missed ({reason:?})")]
    Missed { reason: Resync },
}

pub(crate) enum Inner {
    Desired(Observer),
    Command(Listener),
}

/// Holds the claim on an input for its session and delivers what clients
/// submit. Created by [`Session::control`](crate::Session::control); the
/// session re-claims the input after reconnecting. Dropping the controller
/// stops re-claiming; the claim itself lasts until the session disconnects.
pub struct Controller<T, K: InputKind> {
    id: u64,
    topic: TopicPath,
    inner: Inner,
    claim: watch::Receiver<ClaimState>,
    shared: Weak<Shared>,
    stopped: bool,
    // Desired: the provenance of the last reported request.
    last_write: Option<crate::protocol::ProvenanceView>,
    // Commands: the latest known claim and occurrences not yet returned.
    node_claim: Option<(u64, Option<ClaimView>)>,
    buffered: VecDeque<(u64, ChangeView)>,
    marker: PhantomData<fn() -> (T, K)>,
}

impl<T, K: InputKind> Controller<T, K> {
    pub(crate) fn new(
        id: u64,
        topic: &Topic<T, K>,
        inner: Inner,
        claim: watch::Receiver<ClaimState>,
        shared: Weak<Shared>,
    ) -> Self {
        Self {
            id,
            topic: topic.path().clone(),
            inner,
            claim,
            shared,
            stopped: false,
            last_write: None,
            node_claim: None,
            buffered: VecDeque::new(),
            marker: PhantomData,
        }
    }

    #[must_use]
    pub fn topic(&self) -> &TopicPath {
        &self.topic
    }

    /// Compares the observed claim with the one this session holds.
    /// `None` while undecided: no claim yet (reconnecting), or the observed
    /// state predates the claiming commit.
    fn check_claim(
        &mut self,
        observed_at: u64,
        observed: Option<&ClaimView>,
    ) -> Option<Result<(), ControlError>> {
        let result = match &*self.claim.borrow() {
            ClaimState::Pending => return None,
            ClaimState::Failed(error) => Err(ControlError::ClaimRejected {
                topic: self.topic.clone(),
                error: error.clone(),
            }),
            ClaimState::Held { sequence, .. } if observed_at < *sequence => return None,
            ClaimState::Held { id, .. } if observed.map(|claim| claim.id) == Some(*id) => Ok(()),
            ClaimState::Held { .. } => Err(ControlError::ClaimLost {
                topic: self.topic.clone(),
                by: observed.map(|claim| claim.owner.clone()),
            }),
        };
        self.stopped |= result.is_err();
        Some(result)
    }
}

impl<T: FromValue> Controller<T, Desired> {
    /// The next request, a non-fatal decode error, a fatal claim error (after
    /// which `None` follows), or `None` once the session is closed. The first
    /// call reports a request already pending when control was taken.
    pub async fn recv(&mut self) -> Option<Result<Request<T>, ControlError>> {
        loop {
            if self.stopped {
                return None;
            }
            if let Some(result) = self.evaluate() {
                return Some(result);
            }
            let Inner::Desired(observer) = &mut self.inner else {
                unreachable!("desired controllers observe")
            };
            tokio::select! {
                snapshot = observer.recv() => { snapshot?; }
                changed = self.claim.changed() => changed.ok()?,
            }
        }
    }

    fn evaluate(&mut self) -> Option<Result<Request<T>, ControlError>> {
        // Always judge the newest snapshot: the driver publishes it before
        // updating the claim, so a new claim never meets a stale snapshot.
        let Inner::Desired(observer) = &self.inner else {
            unreachable!("desired controllers observe")
        };
        let snapshot: Snapshot = observer.latest()?;
        let node = snapshot.node(&self.topic).cloned();
        match self.check_claim(snapshot.sequence(), node.as_ref().and_then(NodeView::claim))? {
            Ok(()) => {}
            Err(error) => return Some(Err(error)),
        }
        match node.as_ref().and_then(NodeView::current) {
            Some(current) if self.last_write.as_ref() != Some(&current.last_write) => {
                self.last_write = Some(current.last_write.clone());
                Some(
                    current
                        .decode()
                        .map(|value| {
                            Request::Submitted(Current {
                                value,
                                last_write: current.last_write.clone(),
                                expires_at: current.expires_at,
                            })
                        })
                        .map_err(|source| ControlError::Payload {
                            topic: self.topic.clone(),
                            source,
                        }),
                )
            }
            Some(_) => None,
            None => self.last_write.take().map(|_| Ok(Request::Cleared)),
        }
    }
}

impl<T: FromValue> Controller<T, Command> {
    /// The next command, a non-fatal decode or missed-commands error, a fatal
    /// claim error (after which `None` follows), or `None` once the session
    /// is closed.
    pub async fn recv(&mut self) -> Option<Result<Occurrence<T>, ControlError>> {
        loop {
            if self.stopped {
                return None;
            }
            if let Some((sequence, claim)) = self.node_claim.clone()
                && let Some(Err(error)) = self.check_claim(sequence, claim.as_ref())
            {
                return Some(Err(error));
            }
            if let Some(result) = self.next_buffered() {
                return Some(result);
            }
            let Inner::Command(listener) = &mut self.inner else {
                unreachable!("command controllers listen")
            };
            tokio::select! {
                event = listener.recv() => match event? {
                    Event::Snapshot { snapshot, reason } => {
                        let claim = snapshot.node(&self.topic).and_then(NodeView::claim).cloned();
                        self.node_claim = Some((snapshot.sequence(), claim));
                        if reason != Resync::Started {
                            return Some(Err(ControlError::Missed { reason }));
                        }
                    }
                    Event::Update(update) => {
                        for change in update.changes {
                            match &change {
                                ChangeView::Upsert { node, .. } => {
                                    self.node_claim = Some((update.sequence, node.claim().cloned()));
                                }
                                ChangeView::Removed { .. } => self.node_claim = Some((update.sequence, None)),
                                ChangeView::Command { .. } => self.buffered.push_back((update.sequence, change)),
                                ChangeView::Event { .. } => {}
                            }
                        }
                    }
                },
                changed = self.claim.changed() => changed.ok()?,
            }
        }
    }

    fn next_buffered(&mut self) -> Option<Result<Occurrence<T>, ControlError>> {
        let (sequence, ChangeView::Command { command, .. }) = self.buffered.pop_front()? else {
            return None;
        };
        Some(
            command
                .decode()
                .map(|value| Occurrence {
                    value,
                    provenance: command.provenance,
                    sequence,
                })
                .map_err(|source| ControlError::Payload {
                    topic: self.topic.clone(),
                    source,
                }),
        )
    }
}

impl<T, K: InputKind> Drop for Controller<T, K> {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            shared.unregister(self.id);
        }
    }
}

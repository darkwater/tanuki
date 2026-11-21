//! Every update in order, plus typed event/command occurrences built on it.
use crate::{
    Snapshot, Topic, TopicKind,
    protocol::{ChangeView, FromValue, PayloadError, ProvenanceView, TopicPath, UpdateView},
    session::Shared,
    topic::InstantKind,
};
use std::{
    collections::VecDeque,
    marker::PhantomData,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::Notify;

/// What a [`Listener`] receives.
#[derive(Clone, Debug)]
pub enum Event {
    /// A complete baseline. Apply it instead of everything before it.
    Snapshot { snapshot: Snapshot, reason: Resync },
    /// One complete commit, restricted to the listener's selectors.
    Update(UpdateView),
}

/// Why a [`Listener`] received a fresh baseline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resync {
    /// The first baseline after registering.
    Started,
    /// The connection was re-established; updates (and occurrences) while
    /// disconnected are not replayed.
    Reconnected,
    /// The listener fell further behind than its queue capacity. Queued
    /// updates were replaced by the current state; occurrences were lost.
    Lagged,
}

/// Bounded per-listener queue. The driver never blocks on it: on overflow it
/// replaces the backlog with one `Lagged` baseline.
pub(crate) struct Queue {
    state: Mutex<QueueState>,
    notify: Notify,
    capacity: usize,
}
struct QueueState {
    events: VecDeque<Event>,
    closed: bool,
}

impl Queue {
    pub(crate) fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(QueueState {
                events: VecDeque::new(),
                closed: false,
            }),
            notify: Notify::new(),
            capacity,
        })
    }

    /// Queues `event`, or returns it back when the queue is full.
    pub(crate) fn push(&self, event: Event) -> Result<(), Event> {
        let mut state = self.state.lock().expect("listener queue lock");
        if state.events.len() >= self.capacity {
            return Err(event);
        }
        state.events.push_back(event);
        drop(state);
        self.notify.notify_one();
        Ok(())
    }

    /// Replaces any backlog with one baseline.
    pub(crate) fn reset(&self, event: Event) {
        let mut state = self.state.lock().expect("listener queue lock");
        state.events.clear();
        state.events.push_back(event);
        drop(state);
        self.notify.notify_one();
    }

    pub(crate) fn close(&self) {
        self.state.lock().expect("listener queue lock").closed = true;
        self.notify.notify_one();
    }

    async fn pop(&self) -> Option<Event> {
        loop {
            {
                let mut state = self.state.lock().expect("listener queue lock");
                if let Some(event) = state.events.pop_front() {
                    return Some(event);
                }
                if state.closed {
                    return None;
                }
            }
            // A single consumer: `notify_one` stores a permit if it fires
            // between the check above and this wait.
            self.notify.notified().await;
        }
    }
}

/// Receives a baseline [`Snapshot`] and then every commit touching its
/// selectors, in order. Unlike an [`Observer`](crate::Observer) it sees
/// event and command occurrences. Dropping it unregisters it.
pub struct Listener {
    pub(crate) id: u64,
    pub(crate) queue: Arc<Queue>,
    pub(crate) shared: Weak<Shared>,
}

impl Listener {
    /// The next event, or `None` once the session is closed.
    pub async fn recv(&mut self) -> Option<Event> {
        self.queue.pop().await
    }

    /// Adapts the listener to a standard `Stream`; pin it before polling.
    pub fn into_stream(self) -> impl futures_util::Stream<Item = Event> {
        futures_util::stream::unfold(self, |mut listener| async move {
            listener.recv().await.map(|event| (event, listener))
        })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            shared.unregister(self.id);
        }
    }
}

/// One event or command occurrence.
#[derive(Clone, Debug, PartialEq)]
pub struct Occurrence<T> {
    pub value: T,
    pub provenance: ProvenanceView,
    pub sequence: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum OccurrenceError {
    /// Occurrences may have been lost: after a reconnect, or because the
    /// reader fell too far behind.
    #[error("occurrences may have been missed ({reason:?})")]
    Missed { reason: Resync },
    #[error("could not decode occurrence at {topic}: {source}")]
    Payload {
        topic: TopicPath,
        #[source]
        source: PayloadError,
    },
}

/// Typed occurrences of one event or command topic.
pub struct Occurrences<T, K: TopicKind> {
    listener: Listener,
    topic: TopicPath,
    buffered: VecDeque<(u64, ChangeView)>,
    marker: PhantomData<fn() -> (T, K)>,
}

impl<T: FromValue, K: InstantKind> Occurrences<T, K> {
    pub(crate) fn new(listener: Listener, topic: &Topic<T, K>) -> Self {
        Self {
            listener,
            topic: topic.path().clone(),
            buffered: VecDeque::new(),
            marker: PhantomData,
        }
    }

    /// The next occurrence, an error that occurrences may have been missed or
    /// could not be decoded (both non-fatal), or `None` once the session is
    /// closed.
    pub async fn recv(&mut self) -> Option<Result<Occurrence<T>, OccurrenceError>> {
        loop {
            while let Some((sequence, change)) = self.buffered.pop_front() {
                let (ChangeView::Event {
                    event: occurrence, ..
                }
                | ChangeView::Command {
                    command: occurrence,
                    ..
                }) = change
                else {
                    continue;
                };
                return Some(
                    occurrence
                        .decode()
                        .map(|value| Occurrence {
                            value,
                            provenance: occurrence.provenance,
                            sequence,
                        })
                        .map_err(|source| OccurrenceError::Payload {
                            topic: self.topic.clone(),
                            source,
                        }),
                );
            }
            match self.listener.recv().await? {
                Event::Snapshot {
                    reason: Resync::Started,
                    ..
                } => {}
                Event::Snapshot { reason, .. } => {
                    return Some(Err(OccurrenceError::Missed { reason }));
                }
                Event::Update(update) => self.buffered.extend(
                    update
                        .changes
                        .into_iter()
                        .filter(|change| change.topic() == &self.topic)
                        .map(|change| (update.sequence, change)),
                ),
            }
        }
    }
}

//! The event queue between the driver and [`Events`](super::Events).
//!
//! The driver must never wait on the consumer: it also answers pings and
//! resolves writes. So the queue is bounded for updates only. When updates
//! would exceed the capacity, every queued update is dropped and the driver
//! asks the server for a fresh snapshot (`Snapshot { cause: Resync }`).
//! Snapshots, disconnects and other lifecycle events are rare and always kept.
use super::Event;
use futures_util::task::AtomicWaker;
use std::{
    collections::VecDeque,
    sync::{Mutex, PoisonError},
    task::{Context, Poll},
};

pub(crate) struct Queue {
    state: Mutex<State>,
    waker: AtomicWaker,
    capacity: usize,
}

struct State {
    events: VecDeque<Event>,
    updates: usize,
    /// The driver has finished; no further events will arrive.
    finished: bool,
    /// The consumer dropped [`Events`](super::Events); discard instead of queueing.
    abandoned: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Pushed {
    Queued,
    /// Queued updates were dropped and this one was not queued.
    Overflowed,
}

impl Queue {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(State {
                events: VecDeque::new(),
                updates: 0,
                finished: false,
                abandoned: false,
            }),
            waker: AtomicWaker::new(),
            capacity,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // Nothing panics while holding the lock; recover rather than cascade.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn push(&self, event: Event) -> Pushed {
        let mut state = self.lock();
        if state.abandoned {
            return Pushed::Queued;
        }
        if matches!(event, Event::Update(_)) {
            if state.updates >= self.capacity {
                state
                    .events
                    .retain(|event| !matches!(event, Event::Update(_)));
                state.updates = 0;
                return Pushed::Overflowed;
            }
            state.updates += 1;
        }
        state.events.push_back(event);
        drop(state);
        self.waker.wake();
        Pushed::Queued
    }

    pub(crate) fn finish(&self) {
        self.lock().finished = true;
        self.waker.wake();
    }

    pub(crate) fn abandon(&self) {
        let mut state = self.lock();
        state.abandoned = true;
        state.events.clear();
        state.updates = 0;
    }

    pub(crate) fn poll_next(&self, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        // Register first so a push between the check and returning Pending
        // still wakes this task.
        self.waker.register(cx.waker());
        let mut state = self.lock();
        match state.events.pop_front() {
            Some(event) => {
                if matches!(event, Event::Update(_)) {
                    state.updates -= 1;
                }
                Poll::Ready(Some(event))
            }
            None if state.finished => Poll::Ready(None),
            None => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{SnapshotView, UpdateView};
    use futures_util::task::noop_waker_ref;

    fn update(sequence: u64) -> Event {
        Event::Update(UpdateView {
            sequence,
            changes: vec![],
        })
    }

    fn drain(queue: &Queue) -> Vec<Event> {
        let mut cx = Context::from_waker(noop_waker_ref());
        let mut events = Vec::new();
        while let Poll::Ready(Some(event)) = queue.poll_next(&mut cx) {
            events.push(event);
        }
        events
    }

    #[test]
    fn overflow_drops_queued_updates_but_keeps_lifecycle_events() {
        let queue = Queue::new(2);
        queue.push(Event::Snapshot {
            snapshot: SnapshotView {
                sequence: 0,
                nodes: Default::default(),
            },
            warnings: vec![],
            cause: super::super::SnapshotCause::Connected,
        });
        assert_eq!(queue.push(update(1)), Pushed::Queued);
        assert_eq!(queue.push(update(2)), Pushed::Queued);
        assert_eq!(queue.push(update(3)), Pushed::Overflowed);
        let events = drain(&queue);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], Event::Snapshot { .. }));
        // Capacity is available again after the overflow.
        assert_eq!(queue.push(update(4)), Pushed::Queued);
        assert!(matches!(drain(&queue)[..], [Event::Update(_)]));
    }

    #[test]
    fn consuming_updates_frees_capacity_and_finish_ends_the_stream() {
        let queue = Queue::new(1);
        assert_eq!(queue.push(update(1)), Pushed::Queued);
        assert_eq!(drain(&queue).len(), 1);
        assert_eq!(queue.push(update(2)), Pushed::Queued);
        queue.finish();
        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(matches!(queue.poll_next(&mut cx), Poll::Ready(Some(_))));
        assert!(matches!(queue.poll_next(&mut cx), Poll::Ready(None)));
    }
}

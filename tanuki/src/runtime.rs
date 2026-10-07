//! Runtime coordination around the authoritative core; no socket or disk I/O.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use thiserror::Error;
use tokio::sync::watch;
use tracing::warn;

use crate::{
    core::{
        CommitOutcome, Core, CoreError, LinkInstallOutcome, LinkRemovalOutcome, OpenSessionOutcome,
        SchemaInstallMode, SchemaInstallOutcome, Snapshot, Subscription, SubscriptionCapacity,
    },
    domain::{ClientName, Selection, SessionHandle, SessionId, WriteBatch, WriteContext},
    link::{LinkDefinition, LinkName},
    scheduler::DeadlineScheduler,
    schema::Schema,
};

pub use crate::scheduler::Clock;
pub type SharedCore = Arc<Mutex<Core>>;

/// The server owns this value until all streams and the scheduler have stopped.
pub struct Runtime {
    handle: RuntimeHandle,
}

impl Runtime {
    pub fn start(core: SharedCore, clock: Clock) -> Self {
        let scheduler = DeadlineScheduler::start(core.clone(), clock.clone());
        let (stopped, _) = watch::channel(false);
        let (active, _) = watch::channel(0);
        Self {
            handle: RuntimeHandle(Arc::new(RuntimeState {
                core,
                clock,
                scheduler,
                stopped,
                active,
                coordination: Mutex::new(Coordination::default()),
            })),
        }
    }

    pub fn handle(&self) -> RuntimeHandle {
        self.handle.clone()
    }

    pub async fn shutdown(self) -> Result<(), tokio::task::JoinError> {
        self.handle.begin_shutdown();
        let mut active = self.handle.0.active.subscribe();
        let _ = active.wait_for(|count| *count == 0).await;
        self.handle.0.scheduler.shutdown().await
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.handle.begin_shutdown();
        self.handle.0.scheduler.abort();
    }
}

#[derive(Clone)]
pub struct RuntimeHandle(Arc<RuntimeState>);

struct RuntimeState {
    core: SharedCore,
    clock: Clock,
    scheduler: DeadlineScheduler,
    coordination: Mutex<Coordination>,
    stopped: watch::Sender<bool>,
    active: watch::Sender<usize>,
}

#[derive(Default)]
struct Coordination {
    stopping: bool,
    connections: BTreeMap<ClientName, ActiveConnection>,
}

struct ActiveConnection {
    session: SessionId,
    kick: watch::Sender<bool>,
}

impl RuntimeHandle {
    pub fn begin_shutdown(&self) {
        // Admission, core calls, and replacement all follow coordination -> core.
        // Even if poisoned, signal streams so shutdown can reclaim their tasks.
        let mut coordination = self
            .0
            .coordination
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        coordination.stopping = true;
        self.0.stopped.send_replace(true);
    }

    pub(crate) fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.0.stopped.subscribe()
    }

    fn with_core<T>(
        &self,
        operation: impl FnOnce(&mut Core, crate::domain::Timestamp) -> Result<T, CoreError>,
    ) -> Result<T, RuntimeError> {
        let coordination = self
            .0
            .coordination
            .lock()
            .map_err(|_| RuntimeError::Poisoned)?;
        if coordination.stopping {
            return Err(RuntimeError::Stopping);
        }
        let mut core = self.0.core.lock().map_err(|_| RuntimeError::Poisoned)?;
        Ok(operation(&mut core, (self.0.clock)())?)
    }

    fn mutate<T>(
        &self,
        operation: impl FnOnce(&mut Core, crate::domain::Timestamp) -> Result<T, CoreError>,
    ) -> Result<T, RuntimeError> {
        self.with_core(|core, now| {
            let result = operation(core, now)?;
            self.0.scheduler.rescan();
            Ok(result)
        })
    }

    pub fn apply(
        &self,
        actor: &WriteContext,
        batch: WriteBatch,
    ) -> Result<CommitOutcome, RuntimeError> {
        self.mutate(|core, now| core.apply(actor, batch, now))
    }

    pub fn install_schema(
        &self,
        schema: Schema,
        mode: SchemaInstallMode,
    ) -> Result<SchemaInstallOutcome, RuntimeError> {
        self.mutate(|core, now| core.install_schema(schema, mode, now))
    }

    pub fn install_link(
        &self,
        definition: LinkDefinition,
    ) -> Result<LinkInstallOutcome, RuntimeError> {
        self.mutate(|core, now| core.install_link(definition, now))
    }

    pub fn remove_link(&self, name: &LinkName) -> Result<LinkRemovalOutcome, RuntimeError> {
        self.mutate(|core, now| core.remove_link(name, now))
    }

    pub fn read(&self, selection: &Selection) -> Result<Snapshot, RuntimeError> {
        self.with_core(|core, _| Ok(core.read(selection)))
    }

    pub fn subscribe(
        &self,
        selection: Selection,
        capacity: SubscriptionCapacity,
    ) -> Result<Subscription, RuntimeError> {
        self.with_core(|core, _| core.subscribe(selection, capacity))
    }

    /// Register before handing a stream/upgrade to Axum, including idle hellos.
    pub(crate) fn track_stream(&self) -> Result<StreamLease, RuntimeError> {
        let coordination = self
            .0
            .coordination
            .lock()
            .map_err(|_| RuntimeError::Poisoned)?;
        if coordination.stopping {
            return Err(RuntimeError::Stopping);
        }
        self.0.active.send_modify(|count| *count += 1);
        Ok(StreamLease {
            runtime: self.clone(),
        })
    }

    pub(crate) fn open_connection(
        &self,
        client: ClientName,
        selection: Selection,
        capacity: SubscriptionCapacity,
    ) -> Result<OpenedConnection, RuntimeError> {
        let mut coordination = self
            .0
            .coordination
            .lock()
            .map_err(|_| RuntimeError::Poisoned)?;
        if coordination.stopping {
            return Err(RuntimeError::Stopping);
        }
        let mut core = self.0.core.lock().map_err(|_| RuntimeError::Poisoned)?;
        let (opened, subscription) =
            core.open_session_and_subscribe(client.clone(), selection, capacity, (self.0.clock)())?;
        self.0.scheduler.schedule_claims(opened.pending_releases());
        self.0.scheduler.rescan();
        let (kick, kicked) = watch::channel(false);
        if let Some(previous) = coordination.connections.insert(
            client,
            ActiveConnection {
                session: opened.handle().id(),
                kick,
            },
        ) {
            previous.kick.send_replace(true);
        }
        Ok(OpenedConnection {
            session: SessionLease {
                runtime: self.clone(),
                handle: opened.handle().clone(),
            },
            opened,
            subscription,
            kicked,
        })
    }

    fn disconnect(&self, handle: &SessionHandle) {
        let mut coordination = self
            .0
            .coordination
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match self.0.core.lock() {
            Ok(mut core) => match core.disconnect(handle, (self.0.clock)()) {
                Ok(outcome) => {
                    for warning in outcome.warnings() {
                        warn!(?warning, client = %handle.client(), "disconnect completed with diagnostic");
                    }
                    self.0.scheduler.schedule_claims(outcome.pending_releases());
                    self.0.scheduler.rescan();
                }
                Err(error) => warn!(%error, client = %handle.client(), "session cleanup failed"),
            },
            Err(_) => {
                warn!(client = %handle.client(), "session cleanup failed because core lock was poisoned")
            }
        }
        if coordination
            .connections
            .get(handle.client())
            .is_some_and(|connection| connection.session == handle.id())
        {
            coordination.connections.remove(handle.client());
        }
    }
}

pub(crate) struct OpenedConnection {
    pub opened: OpenSessionOutcome,
    pub subscription: Subscription,
    pub kicked: watch::Receiver<bool>,
    pub session: SessionLease,
}

pub(crate) struct SessionLease {
    runtime: RuntimeHandle,
    handle: SessionHandle,
}

impl SessionLease {
    pub fn handle(&self) -> &SessionHandle {
        &self.handle
    }
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        self.runtime.disconnect(&self.handle);
    }
}

pub(crate) struct StreamLease {
    runtime: RuntimeHandle,
}

impl Drop for StreamLease {
    fn drop(&mut self) {
        self.runtime.0.active.send_modify(|count| *count -= 1);
    }
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("server is shutting down")]
    Stopping,
    #[error("server state lock was poisoned")]
    Poisoned,
    #[error(transparent)]
    Core(#[from] CoreError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ExpiryUpdate, Timestamp, TopicPath, Value, WriteOperation};

    fn clock() -> Clock {
        Arc::new(|| Timestamp::new(jiff::Timestamp::from_second(0).unwrap()))
    }

    fn write() -> WriteBatch {
        WriteBatch::new(vec![WriteOperation::PublishState {
            topic: TopicPath::parse("/test").unwrap(),
            value: Value::Integer(1),
            expiry: ExpiryUpdate::Clear,
        }])
        .unwrap()
    }

    #[tokio::test]
    async fn concurrent_replacements_keep_the_newest_core_session_and_socket_registered() {
        let core = Arc::new(Mutex::new(Core::new()));
        let runtime = Runtime::start(core.clone(), clock());
        let handle = runtime.handle();
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let mut connections = std::thread::scope(|scope| {
            let threads = (0..16)
                .map(|_| {
                    let handle = handle.clone();
                    let barrier = barrier.clone();
                    scope.spawn(move || {
                        barrier.wait();
                        handle
                            .open_connection(
                                ClientName::parse("controller").unwrap(),
                                Selection::new(vec![]),
                                SubscriptionCapacity::new(8).unwrap(),
                            )
                            .unwrap()
                    })
                })
                .collect::<Vec<_>>();
            threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect::<Vec<_>>()
        });
        connections.sort_by_key(|connection| connection.session.handle().id().get());
        let current = connections.pop().unwrap();
        assert!(!*current.kicked.borrow());
        for displaced in &connections {
            assert!(*displaced.kicked.borrow());
            assert!(matches!(
                handle.apply(
                    &WriteContext::managed(displaced.session.handle().clone()),
                    write()
                ),
                Err(RuntimeError::Core(CoreError::SessionExpired { .. }))
            ));
        }
        {
            let registry = handle.0.coordination.lock().unwrap();
            assert_eq!(registry.connections.len(), 1);
            assert_eq!(
                registry.connections.values().next().unwrap().session,
                current.session.handle().id()
            );
        }
        drop(connections);
        assert_eq!(core.lock().unwrap().managed_session_count(), 1);
        handle
            .apply(
                &WriteContext::managed(current.session.handle().clone()),
                write(),
            )
            .unwrap();
        drop(current);
        runtime.shutdown().await.unwrap();
        assert_eq!(core.lock().unwrap().managed_session_count(), 0);
    }

    #[tokio::test]
    async fn shutdown_closes_admission_even_while_runtime_handles_remain_alive() {
        let core = Arc::new(Mutex::new(Core::new()));
        let runtime = Runtime::start(core.clone(), clock());
        let handle = runtime.handle();
        handle.begin_shutdown();
        assert!(matches!(handle.track_stream(), Err(RuntimeError::Stopping)));
        assert!(matches!(
            handle.open_connection(
                ClientName::parse("late").unwrap(),
                Selection::new(vec![]),
                SubscriptionCapacity::new(8).unwrap()
            ),
            Err(RuntimeError::Stopping)
        ));
        runtime.shutdown().await.unwrap();
        assert!(matches!(
            handle.apply(
                &WriteContext::stateless(ClientName::parse("late").unwrap()),
                write()
            ),
            Err(RuntimeError::Stopping)
        ));
        assert_eq!(core.lock().unwrap().managed_session_count(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_runtime_stops_timers_even_when_a_handle_is_retained() {
        use std::sync::atomic::{AtomicI64, Ordering};
        let core = Arc::new(Mutex::new(Core::new()));
        let second = Arc::new(AtomicI64::new(0));
        let wall_second = second.clone();
        let clock: Clock = Arc::new(move || {
            Timestamp::new(
                jiff::Timestamp::from_second(wall_second.load(Ordering::SeqCst)).unwrap(),
            )
        });
        let runtime = Runtime::start(core.clone(), clock);
        let handle = runtime.handle();
        handle
            .apply(
                &WriteContext::stateless(ClientName::parse("publisher").unwrap()),
                WriteBatch::new(vec![WriteOperation::PublishState {
                    topic: TopicPath::parse("/retained").unwrap(),
                    value: Value::Integer(1),
                    expiry: ExpiryUpdate::Set(
                        crate::domain::NonNegativeDuration::new(jiff::SignedDuration::from_secs(
                            10,
                        ))
                        .unwrap(),
                    ),
                }])
                .unwrap(),
            )
            .unwrap();
        tokio::task::yield_now().await;
        drop(runtime);
        second.store(20, Ordering::SeqCst);
        tokio::time::advance(std::time::Duration::from_secs(20)).await;
        tokio::task::yield_now().await;
        assert!(
            core.lock()
                .unwrap()
                .read(&Selection::new(vec![
                    crate::domain::Selector::parse("/**").unwrap()
                ]))
                .get(&TopicPath::parse("/retained").unwrap())
                .is_some()
        );
        assert!(matches!(
            handle.apply(
                &WriteContext::stateless(ClientName::parse("late").unwrap()),
                write()
            ),
            Err(RuntimeError::Stopping)
        ));
    }
}

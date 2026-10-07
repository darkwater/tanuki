mod listener;

use std::{
    future::{Future, IntoFuture},
    io,
    sync::{Arc, Mutex},
};

use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::warn;

use crate::{
    core::PersistenceSnapshot,
    domain::Timestamp,
    persistence::{PersistenceError, SnapshotStore},
    runtime::{Clock, Runtime, RuntimeHandle, SharedCore},
    transport::router,
};

/// Serves the production HTTP router on an already-bound listener.
pub async fn serve_until<S>(listener: TcpListener, shutdown: S) -> Result<(), ServerError>
where
    S: Future<Output = ()> + Send + 'static,
{
    let clock: Clock = Arc::new(|| Timestamp::new(jiff::Timestamp::now()));
    let path = std::env::var_os("TANUKI_SNAPSHOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("tanuki.db"));
    serve_with_persistence(
        listener,
        SnapshotStore::new(path),
        clock,
        std::time::Duration::from_secs(30),
        shutdown,
    )
    .await
}

/// Serves with injected state and time for whole-system tests.
pub async fn serve_with_core<S>(
    listener: TcpListener,
    core: SharedCore,
    clock: Clock,
    shutdown: S,
) -> io::Result<()>
where
    S: Future<Output = ()> + Send + 'static,
{
    let runtime = Runtime::start(core, clock);
    let serve = serve_runtime(listener, runtime.handle(), shutdown).await;
    runtime.shutdown().await.map_err(io::Error::other)?;
    serve
}

pub async fn serve_with_persistence<S>(
    listener: TcpListener,
    store: SnapshotStore,
    clock: Clock,
    interval: std::time::Duration,
    shutdown: S,
) -> Result<(), ServerError>
where
    S: Future<Output = ()> + Send + 'static,
{
    let loaded = store.load_or_recover(clock())?;
    if let Some(recovery) = loaded.recovery() {
        warn!(
            backup = %recovery.backup().display(),
            reason = recovery.reason(),
            "snapshot was invalid; backed it up and started with empty state"
        );
    }
    let core = Arc::new(Mutex::new(loaded.into_core()));
    let runtime = Runtime::start(Arc::clone(&core), Arc::clone(&clock));
    let (stop, stopped) = watch::channel(false);
    let saver = tokio::spawn(periodic_save(
        store.clone(),
        Arc::clone(&core),
        Arc::clone(&clock),
        interval,
        stopped,
    ));
    let serve = serve_runtime(listener, runtime.handle(), async move {
        shutdown.await;
        stop.send_replace(true);
    })
    .await;
    runtime
        .shutdown()
        .await
        .map_err(ServerError::SchedulerTask)?;
    saver.await.map_err(ServerError::SnapshotTask)?;
    serve.map_err(ServerError::Serve)?;
    store.save(capture_snapshot(&core)?, clock()).await?;
    Ok(())
}

async fn serve_runtime<S>(
    listener: TcpListener,
    runtime: RuntimeHandle,
    shutdown: S,
) -> io::Result<()>
where
    S: Future<Output = ()> + Send + 'static,
{
    let (force, forced) = watch::channel(false);
    let mut stopped = runtime.shutdown_signal();
    let handle = runtime.clone();
    let serve = axum::serve(
        listener::ShutdownListener::new(listener, forced),
        router(runtime),
    )
    .with_graceful_shutdown(async move {
        shutdown.await;
        handle.begin_shutdown();
    })
    .into_future();
    tokio::pin!(serve);
    tokio::select! {
        result = &mut serve => result,
        () = async {
            let _ = stopped.wait_for(|stopped| *stopped).await;
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        } => {
            warn!("shutdown drain window ended; closing remaining sockets");
            force.send_replace(true);
            serve.await
        }
    }
}

async fn periodic_save(
    store: SnapshotStore,
    core: SharedCore,
    clock: Clock,
    interval: std::time::Duration,
    mut stopped: watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
    loop {
        tokio::select! {
            result = stopped.changed() => {
                if result.is_err() || *stopped.borrow() {
                    return;
                }
            }
            _ = ticker.tick() => {
                let result = match capture_snapshot(&core) {
                    Ok(snapshot) => store.save(snapshot, clock()).await.map_err(ServerError::from),
                    Err(error) => Err(error),
                };
                if let Err(error) = result {
                    warn!(%error, "periodic snapshot save failed");
                }
            }
        }
    }
}

fn capture_snapshot(core: &SharedCore) -> Result<PersistenceSnapshot, ServerError> {
    Ok(core
        .lock()
        .map_err(|_| ServerError::CorePoisoned)?
        .persistence_snapshot())
}

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("core state lock was poisoned during snapshot capture")]
    CorePoisoned,
    #[error("deadline scheduler task failed")]
    SchedulerTask(#[source] tokio::task::JoinError),
    #[error("server I/O failed")]
    Serve(#[source] io::Error),
    #[error("snapshot persistence failed")]
    Persistence(#[from] PersistenceError),
    #[error("snapshot task failed")]
    SnapshotTask(#[source] tokio::task::JoinError),
}

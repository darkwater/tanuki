use std::{
    future::Future,
    io,
    sync::{Arc, Mutex},
};

use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::warn;

use crate::{
    domain::Timestamp,
    persistence::{PersistenceError, SnapshotStore},
    transport::{Clock, SharedCore, router_with_clock},
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
    axum::serve(listener, router_with_clock(core, clock))
        .with_graceful_shutdown(shutdown)
        .await
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
    let (stop, stopped) = watch::channel(false);
    let saver = tokio::spawn(periodic_save(
        store.clone(),
        Arc::clone(&core),
        Arc::clone(&clock),
        interval,
        stopped,
    ));
    let serve = axum::serve(
        listener,
        router_with_clock(Arc::clone(&core), Arc::clone(&clock)),
    )
    .with_graceful_shutdown(async move {
        shutdown.await;
        stop.send_replace(true);
    })
    .await;
    saver.await.map_err(ServerError::SnapshotTask)?;
    serve.map_err(ServerError::Serve)?;
    store.save(core, clock()).await?;
    Ok(())
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
                if let Err(error) = store.save(Arc::clone(&core), clock()).await {
                    warn!(%error, "periodic snapshot save failed");
                }
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("server I/O failed")]
    Serve(#[source] io::Error),
    #[error("snapshot persistence failed")]
    Persistence(#[from] PersistenceError),
    #[error("snapshot task failed")]
    SnapshotTask(#[source] tokio::task::JoinError),
}

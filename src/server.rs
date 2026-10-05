use std::{
    future::Future,
    io,
    sync::{Arc, Mutex},
};

use tokio::net::TcpListener;

use crate::{
    core::Core,
    domain::Timestamp,
    transport::{Clock, SharedCore, router_with_clock},
};

/// Serves the production HTTP router on an already-bound listener.
pub async fn serve_until<S>(listener: TcpListener, shutdown: S) -> io::Result<()>
where
    S: Future<Output = ()> + Send + 'static,
{
    let core = Arc::new(Mutex::new(Core::new()));
    let clock: Clock = Arc::new(|| Timestamp::new(jiff::Timestamp::now()));
    serve_with_core(listener, core, clock, shutdown).await
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

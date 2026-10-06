use std::sync::{Arc, Mutex};

use jiff::Timestamp as JiffTimestamp;
use tanuki::{core::Core, domain::Timestamp, transport::Clock};
use tokio::sync::oneshot;

#[tokio::test]
async fn server_stops_when_shutdown_is_requested() {
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let clock: Clock = Arc::new(|| Timestamp::new(JiffTimestamp::from_second(0).unwrap()));
    let server = tokio::spawn(tanuki::server::serve_with_core(
        listener,
        Arc::new(Mutex::new(Core::new())),
        clock,
        async move {
            let _ = shutdown_rx.await;
        },
    ));

    shutdown_tx
        .send(())
        .expect("server still receives shutdown");
    server
        .await
        .expect("server task joins")
        .expect("server stops cleanly");
}

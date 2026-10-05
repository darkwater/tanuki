use tokio::sync::oneshot;

#[tokio::test]
async fn server_stops_when_shutdown_is_requested() {
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(tanuki::server::run_until(shutdown_rx));

    shutdown_tx
        .send(())
        .expect("server still receives shutdown");
    server
        .await
        .expect("server task joins")
        .expect("server stops cleanly");
}

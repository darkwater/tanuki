use tokio::sync::oneshot;

#[tokio::test]
async fn server_stops_when_shutdown_is_requested() {
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = tokio::spawn(tanuki::server::serve_until(listener, async move {
        let _ = shutdown_rx.await;
    }));

    shutdown_tx
        .send(())
        .expect("server still receives shutdown");
    server
        .await
        .expect("server task joins")
        .expect("server stops cleanly");
}

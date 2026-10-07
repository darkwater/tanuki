use std::sync::{Arc, Mutex};

use jiff::Timestamp as JiffTimestamp;
use tanuki::{core::Core, domain::Timestamp, transport::Clock};
use tokio::sync::oneshot;

use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::{Duration, timeout},
};

#[tokio::test]
async fn shutdown_drains_idle_websocket_sessions_and_prevents_later_writes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let core = Arc::new(Mutex::new(Core::new()));
    let clock: Clock = Arc::new(|| Timestamp::new(JiffTimestamp::from_second(0).unwrap()));
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(tanuki::server::serve_with_core(
        listener,
        core.clone(),
        clock,
        async {
            let _ = stopped.await;
        },
    ));
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/v1/ws"))
        .await
        .unwrap();
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::json!({
                "type": "hello", "request_id": "hello", "client": "controller", "selectors": []
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let snapshot = timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(snapshot.to_text().unwrap().contains("snapshot"));
    stop.send(()).unwrap();
    timeout(Duration::from_secs(2), server)
        .await
        .expect("shutdown must drain sockets")
        .unwrap()
        .unwrap();
    assert_eq!(
        core.lock().unwrap().managed_session_count(),
        0,
        "session survived server shutdown"
    );
    let before = core
        .lock()
        .unwrap()
        .read(&tanuki::domain::Selection::new(vec![]))
        .sequence();
    let _ = socket.send(tokio_tungstenite::tungstenite::Message::Text(serde_json::json!({
        "type": "write", "request_id": "late", "operations": [{"op": "publish_state", "topic": "/late", "value": 1}]
    }).to_string().into())).await;
    let closed = timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap();
    assert!(!matches!(
        closed,
        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(_)))
    ));
    assert_eq!(
        core.lock()
            .unwrap()
            .read(&tanuki::domain::Selection::new(vec![]))
            .sequence(),
        before
    );
}

#[tokio::test]
async fn shutdown_finishes_with_an_idle_sse_client_still_connected() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let core = Arc::new(Mutex::new(Core::new()));
    let clock: Clock = Arc::new(|| Timestamp::new(JiffTimestamp::from_second(0).unwrap()));
    let (stop, stopped) = oneshot::channel();
    let mut server = tokio::spawn(tanuki::server::serve_with_core(
        listener,
        core,
        clock,
        async {
            let _ = stopped.await;
        },
    ));
    let mut socket = TcpStream::connect(address).await.unwrap();
    socket
        .write_all(format!("GET /v1/sse?select=/** HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut bytes = vec![0; 4096];
    let count = timeout(Duration::from_secs(2), socket.read(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&bytes[..count]).contains("200 OK"));
    stop.send(()).unwrap();
    let result = timeout(Duration::from_secs(2), &mut server).await;
    if result.is_err() {
        server.abort();
    }
    result
        .expect("SSE must not hold shutdown open")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn shutdown_bounds_sse_drain_when_the_peer_stops_reading() {
    use tanuki::domain::{
        ClientName, ExpiryUpdate, TopicPath, Value, WriteBatch, WriteContext, WriteOperation,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let core = Arc::new(Mutex::new(Core::new()));
    let clock: Clock = Arc::new(|| Timestamp::new(JiffTimestamp::from_second(0).unwrap()));
    let (stop, stopped) = oneshot::channel();
    let mut server = tokio::spawn(tanuki::server::serve_with_core(
        listener,
        core.clone(),
        clock,
        async {
            let _ = stopped.await;
        },
    ));
    let mut socket = TcpStream::connect(address).await.unwrap();
    socket
        .write_all(format!("GET /v1/sse?select=/** HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut bytes = vec![0; 4096];
    let count = timeout(Duration::from_secs(2), socket.read(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&bytes[..count]).contains("200 OK"));
    let actor = WriteContext::stateless(ClientName::parse("publisher").unwrap());
    // Keep the TCP peer open without reading while the real SSE transport fills
    // its socket buffer. Queues retain complete commits; overflow may close it.
    for second in 0..64 {
        core.lock()
            .unwrap()
            .apply(
                &actor,
                WriteBatch::new(vec![WriteOperation::PublishState {
                    topic: TopicPath::parse("/large").unwrap(),
                    value: Value::String("x".repeat(256 * 1024)),
                    expiry: ExpiryUpdate::Clear,
                }])
                .unwrap(),
                Timestamp::new(JiffTimestamp::from_second(second).unwrap()),
            )
            .unwrap();
        tokio::task::yield_now().await;
    }
    stop.send(()).unwrap();
    let result = timeout(Duration::from_secs(3), &mut server).await;
    if result.is_err() {
        server.abort();
    }
    result
        .expect("a non-reading peer must not hold shutdown open")
        .unwrap()
        .unwrap();
    drop(socket);
}

#[tokio::test]
async fn shutdown_drains_websocket_peers_that_never_send_hello() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let core = Arc::new(Mutex::new(Core::new()));
    let clock: Clock = Arc::new(|| Timestamp::new(JiffTimestamp::from_second(0).unwrap()));
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(tanuki::server::serve_with_core(
        listener,
        core.clone(),
        clock,
        async {
            let _ = stopped.await;
        },
    ));
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/v1/ws"))
        .await
        .unwrap();
    stop.send(()).unwrap();
    timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ended = timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap();
    assert!(!matches!(
        ended,
        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(_)))
    ));
    assert_eq!(core.lock().unwrap().managed_session_count(), 0);
}

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

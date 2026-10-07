use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use futures_util::{SinkExt, StreamExt};
use jiff::Timestamp as JiffTimestamp;
use serde_json::{Value as RawJson, json};
use tanuki::{
    core::Core,
    domain::{
        ClientName, ExpiryUpdate, Timestamp, TopicPath, Value, WriteBatch, WriteContext,
        WriteOperation,
    },
    persistence::SnapshotStore,
    server::serve_with_persistence,
    transport::Clock,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

#[tokio::test]
async fn final_save_includes_acknowledged_websocket_writes_with_the_peer_still_open() {
    let directory = TestDirectory::new();
    let store = SnapshotStore::new(directory.0.join("live-peer.db"));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(serve_with_persistence(
        listener,
        store.clone(),
        clock(),
        std::time::Duration::from_secs(60),
        async {
            let _ = stopped.await;
        },
    ));
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/v1/ws"))
        .await
        .unwrap();
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            json!({
                "type": "hello", "request_id": "hello", "client": "publisher", "selectors": []
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let snapshot = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<RawJson>(snapshot.to_text().unwrap()).unwrap()["type"],
        "snapshot"
    );
    socket.send(tokio_tungstenite::tungstenite::Message::Text(json!({
        "type": "write", "request_id": "last", "operations": [{"op": "publish_state", "topic": "/battery/laptop", "value": 73}]
    }).to_string().into())).await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let reply: RawJson = serde_json::from_str(reply.to_text().unwrap()).unwrap();
    assert_eq!(reply["type"], "reply");
    assert_eq!(reply["request_id"], "last");
    stop.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let restored = store.load(clock()()).unwrap().unwrap();
    let selected = restored.read(&tanuki::domain::Selection::new(vec![
        tanuki::domain::Selector::parse("/**").unwrap(),
    ]));
    assert_eq!(
        selected
            .get(&TopicPath::parse("/battery/laptop").unwrap())
            .unwrap()
            .retained_value()
            .unwrap()
            .value(),
        &Value::Integer(73)
    );
    let ended = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .unwrap();
    assert!(!matches!(
        ended,
        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(_)))
    ));
}

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let unique = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "tanuki-restart-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn clock() -> Clock {
    Arc::new(|| Timestamp::new(JiffTimestamp::from_second(1_700_000_000).unwrap()))
}

async fn request(address: std::net::SocketAddr, request: &str) -> RawJson {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let separator = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    serde_json::from_slice(&response[separator + 4..]).unwrap()
}

#[tokio::test]
async fn orderly_save_and_restart_restore_retained_state_over_http() {
    let directory = TestDirectory::new();
    let store = SnapshotStore::new(directory.0.join("snapshot.db"));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(serve_with_persistence(
        listener,
        store.clone(),
        clock(),
        std::time::Duration::from_secs(60),
        async move {
            let _ = stopped.await;
        },
    ));
    let body = serde_json::to_vec(&json!({"value": 73})).unwrap();
    let write = format!(
        "POST /v1/state/battery/phone HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nTanuki-Client: phone\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        String::from_utf8(body).unwrap()
    );
    assert_eq!(request(address, &write).await["ok"], true);
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(serve_with_persistence(
        listener,
        store,
        clock(),
        std::time::Duration::from_secs(60),
        async move {
            let _ = stopped.await;
        },
    ));
    let read = format!(
        "GET /v1/snapshot?select=/battery/* HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    );
    let response = request(address, &read).await;
    assert_eq!(
        response["data"]["nodes"]["/battery/phone"]["current"]["value"],
        73
    );
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn production_binary_restores_configured_snapshot() {
    let directory = TestDirectory::new();
    let path = directory.0.join("snapshot.db");
    let store = SnapshotStore::new(path.clone());
    let core = Arc::new(std::sync::Mutex::new(Core::new()));
    core.lock()
        .unwrap()
        .apply(
            &WriteContext::stateless(ClientName::parse("fixture").unwrap()),
            WriteBatch::new(vec![WriteOperation::PublishState {
                topic: TopicPath::parse("/battery/restored").unwrap(),
                value: Value::Integer(66),
                expiry: ExpiryUpdate::Clear,
            }])
            .unwrap(),
            Timestamp::new(JiffTimestamp::now()),
        )
        .unwrap();
    let snapshot = core.lock().unwrap().persistence_snapshot();
    store
        .save(snapshot, Timestamp::new(JiffTimestamp::now()))
        .await
        .unwrap();

    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_tanuki"))
        .env("TANUKI_LISTEN", address.to_string())
        .env("TANUKI_SNAPSHOT", &path)
        .spawn()
        .unwrap();
    let connected = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match TcpStream::connect(address).await {
                Ok(stream) => break stream,
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .expect("production binary did not start");
    drop(connected);
    let read = format!(
        "GET /v1/snapshot?select=/battery/* HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    );
    let response = request(address, &read).await;
    assert_eq!(
        response["data"]["nodes"]["/battery/restored"]["current"]["value"],
        66
    );
    child.kill().unwrap();
    child.wait().unwrap();
}

#[tokio::test]
async fn corrupt_snapshot_is_backed_up_and_server_starts_empty() {
    let directory = TestDirectory::new();
    let path = directory.0.join("tanuki.db");
    fs::write(&path, b"broken snapshot").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(serve_with_persistence(
        listener,
        SnapshotStore::new(path.clone()),
        clock(),
        std::time::Duration::from_secs(60),
        async move {
            let _ = stopped.await;
        },
    ));

    let read = format!(
        "GET /v1/snapshot?select=/** HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    );
    assert_eq!(request(address, &read).await["data"]["nodes"], json!({}));
    assert_eq!(
        fs::read(path.with_extension("db.bak")).unwrap(),
        b"broken snapshot"
    );

    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
    assert!(
        SnapshotStore::new(path)
            .load(Timestamp::new(JiffTimestamp::now()))
            .unwrap()
            .is_some()
    );
}

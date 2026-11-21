#![cfg(not(target_arch = "wasm32"))]
//! Scripted peers inject missing, late and malformed server behaviour. The
//! real server's behaviour is covered by tanuki's integration suite.
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tanuki_client::protocol::*;
use tanuki_client::{
    ClientError, Closed, ConnectionError, Disconnect, Reconnect, Session, StateTopic, Status,
};
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle, time::timeout};
use tokio_tungstenite::tungstenite::{
    Message,
    protocol::{CloseFrame, frame::coding::CloseCode},
};

/// Accepts one connection, answers hello with an empty snapshot, reports each
/// write's request id, and sends whatever the test scripts.
struct Peer {
    url: String,
    writes: mpsc::Receiver<RequestId>,
    script: mpsc::Sender<Message>,
    task: JoinHandle<()>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Peer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let (writes, received) = mpsc::channel(8);
        let (script, mut scripted) = mpsc::channel::<Message>(8);
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let Some(Ok(Message::Text(hello))) = socket.next().await else {
                return;
            };
            let ClientMessage::Hello { request_id, .. } = serde_json::from_str(&hello).unwrap()
            else {
                panic!("expected hello");
            };
            let snapshot = ServerMessage::Snapshot {
                request_id,
                sequence: 1,
                nodes: Default::default(),
                warnings: vec![],
            };
            socket
                .send(Message::Text(
                    serde_json::to_string(&snapshot).unwrap().into(),
                ))
                .await
                .unwrap();
            loop {
                tokio::select! {
                    message = socket.next() => match message {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(ClientMessage::Write { request_id, .. }) = serde_json::from_str(&text)
                                && writes.send(request_id).await.is_err()
                            {
                                break;
                            }
                        }
                        Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                        _ => {}
                    },
                    message = scripted.recv() => match message {
                        Some(message) => if socket.send(message).await.is_err() { break; },
                        None => break,
                    },
                }
            }
        });
        Self {
            url,
            writes: received,
            script,
            task,
        }
    }

    async fn send(&self, message: &ServerMessage) {
        self.send_frame(Message::Text(
            serde_json::to_string(message).unwrap().into(),
        ))
        .await;
    }

    async fn send_frame(&self, frame: Message) {
        self.script.send(frame).await.unwrap();
    }

    async fn received(&mut self) -> RequestId {
        timeout(Duration::from_secs(3), self.writes.recv())
            .await
            .expect("the write did not reach the peer")
            .expect("the peer stopped")
    }

    async fn session(&self) -> Session {
        Session::builder(&self.url, "scripted client")
            .reconnect(Reconnect::Never)
            .write_timeout(Duration::from_millis(300))
            .connect()
            .await
            .unwrap()
    }
}

fn reply(request_id: RequestId, sequence: u64) -> ServerMessage {
    ServerMessage::Reply {
        request_id,
        result: serde_json::json!({"sequence": sequence, "warnings": []}),
    }
}

fn level() -> StateTopic<i64> {
    StateTopic::new("/fixture/level").unwrap()
}

async fn closed_reason(session: &Session) -> Closed {
    timeout(Duration::from_secs(3), session.closed())
        .await
        .expect("the session did not end")
}

#[tokio::test]
async fn a_missing_reply_times_out_and_a_late_one_cannot_resolve_the_next_write() {
    let mut peer = Peer::start().await;
    let session = peer.session().await;
    let topic = level();
    let (result, late) = tokio::join!(session.publish(&topic, &1), peer.received());
    assert!(matches!(result, Err(ClientError::ReplyTimeout)));

    let second = tokio::spawn({
        let session = session.clone();
        async move { session.publish(&level(), &2).await }
    });
    let current = peer.received().await;
    peer.send(&reply(late, 10)).await;
    peer.send(&reply(current, 11)).await;
    assert_eq!(second.await.unwrap().unwrap().sequence, 11);
    session.close().await;
}

#[tokio::test]
async fn a_connection_lost_after_sending_leaves_the_outcome_unknown() {
    let mut peer = Peer::start().await;
    let session = peer.session().await;
    let write = tokio::spawn({
        let session = session.clone();
        async move { session.publish(&level(), &1).await }
    });
    peer.received().await;
    peer.task.abort();
    assert!(matches!(
        write.await.unwrap(),
        Err(ClientError::OutcomeUnknown)
    ));
    assert!(matches!(closed_reason(&session).await, Closed::Failed(_)));
    assert!(matches!(
        session.publish(&level(), &2).await,
        Err(ClientError::Closed(Closed::Failed(_)))
    ));
}

#[tokio::test]
async fn an_oversized_write_fails_locally_without_ending_the_session() {
    let peer = Peer::start().await;
    let session = peer.session().await;
    let blob = StateTopic::<String>::new("/fixture/blob").unwrap();
    assert!(matches!(
        session.publish(&blob, &"x".repeat(1024 * 1024)).await,
        Err(ClientError::Encode(ConnectionError::MessageTooLarge))
    ));
    assert!(matches!(session.status().current(), Status::Connected));
    session.close().await;
}

#[tokio::test]
async fn a_sequence_regression_is_a_protocol_failure() {
    let peer = Peer::start().await;
    let session = peer.session().await;
    peer.send(&ServerMessage::Update {
        sequence: 1,
        changes: vec![],
    })
    .await;
    assert!(matches!(
        closed_reason(&session).await,
        Closed::Failed(Disconnect::Protocol(_))
    ));
}

#[tokio::test]
async fn trailing_messagepack_bytes_fail_without_applying_a_partial_message() {
    let peer = Peer::start().await;
    let session = peer.session().await;
    let mut bytes = rmp_serde::to_vec_named(&ServerMessage::Update {
        sequence: 2,
        changes: vec![],
    })
    .unwrap();
    bytes.push(0xc0);
    peer.send_frame(Message::Binary(bytes.into())).await;
    assert!(matches!(
        closed_reason(&session).await,
        Closed::Failed(Disconnect::Connection(error))
            if matches!(*error, ConnectionError::Decode(_))
    ));
}

#[tokio::test]
async fn a_replaced_session_ends_instead_of_reconnecting() {
    let peer = Peer::start().await;
    // Even with reconnects enabled: taking the name back would replace the
    // newer connection in turn.
    let session = Session::builder(&peer.url, "scripted client")
        .reconnect(Reconnect::Backoff {
            initial: Duration::from_millis(10),
            max: Duration::from_millis(10),
        })
        .connect()
        .await
        .unwrap();
    peer.send_frame(Message::Close(Some(CloseFrame {
        code: CloseCode::Library(4001),
        reason: "session_replaced".into(),
    })))
    .await;
    assert!(matches!(closed_reason(&session).await, Closed::Replaced));
}

#[tokio::test]
async fn closing_answers_a_write_in_flight_as_closed() {
    let mut peer = Peer::start().await;
    let session = peer.session().await;
    let write = tokio::spawn({
        let session = session.clone();
        async move { session.publish(&level(), &1).await }
    });
    peer.received().await;
    session.close().await;
    assert!(matches!(
        write.await.unwrap(),
        Err(ClientError::Closed(Closed::ByClient))
    ));
    assert!(matches!(
        session.status().current(),
        Status::Closed(Closed::ByClient)
    ));
}

#[tokio::test]
async fn connect_fails_fast_when_nothing_listens() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    drop(listener);
    assert!(matches!(
        Session::connect(&url, "early client").await,
        Err(ClientError::Connect(Disconnect::Connection(_)))
    ));
}

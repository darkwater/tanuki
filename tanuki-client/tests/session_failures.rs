//! Narrow scripted peers inject missing/late replies. Production server behavior is tested by tanuki's integration suite.
use futures_util::{SinkExt, StreamExt};
use tanuki_client::protocol::*;
use tanuki_client::{ClientError, ConnectionError, ExpiryUpdate, Session, SessionOptions};
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};
use tokio_tungstenite::tungstenite::Message;
struct Peer {
    url: String,
    writes: mpsc::Receiver<RequestId>,
    replies: mpsc::Sender<Message>,
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
        let (writes, rx) = mpsc::channel(8);
        let (replies, mut messages) = mpsc::channel::<Message>(8);
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let Message::Text(hello) = socket.next().await.unwrap().unwrap() else {
                panic!()
            };
            let ClientMessage::Hello { request_id, .. } = serde_json::from_str(&hello).unwrap()
            else {
                panic!()
            };
            socket
                .send(Message::Text(
                    serde_json::to_string(&ServerMessage::Snapshot {
                        request_id,
                        sequence: 0,
                        nodes: Default::default(),
                        warnings: vec![],
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            loop {
                tokio::select! {
                    message = socket.next() => match message {
                        Some(Ok(Message::Text(text))) => { let ClientMessage::Write { request_id, .. } = serde_json::from_str(&text).unwrap() else { panic!() }; if writes.send(request_id).await.is_err() { break; } }
                        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                        _ => {}
                    },
                    message = messages.recv() => match message { Some(message) => { if socket.send(message).await.is_err() { break; } }, None => break }
                }
            }
        });
        Self {
            url,
            writes: rx,
            replies,
            task,
        }
    }
    async fn send(&self, message: ServerMessage) {
        self.replies
            .send(Message::Text(
                serde_json::to_string(&message).unwrap().into(),
            ))
            .await
            .unwrap();
    }
    async fn received(&mut self) -> RequestId {
        tokio::time::timeout(std::time::Duration::from_secs(3), self.writes.recv())
            .await
            .expect("write did not reach scripted peer")
            .unwrap()
    }
    async fn session(&self) -> Session {
        Session::connect_with_options(
            &self.url,
            ClientName::parse("failure probe").unwrap(),
            vec![],
            SessionOptions {
                pending_requests: std::num::NonZeroUsize::new(1).unwrap(),
                write_timeout: std::time::Duration::from_millis(200),
                ..Default::default()
            },
        )
        .await
        .unwrap()
    }
    async fn reply(&self, id: RequestId, sequence: u64) {
        self.send(ServerMessage::Reply {
            request_id: id,
            result: serde_json::json!({"sequence": sequence, "warnings": []}),
        })
        .await;
    }
}
#[tokio::test]
async fn canceled_waiter_releases_capacity_and_late_reply_cannot_resolve_next_request() {
    let mut peer = Peer::start().await;
    let session = peer.session().await;
    let topic = session.state::<u8>("/value").unwrap();
    let first = topic.clone();
    let abandoned = tokio::spawn(async move { first.publish(&1, ExpiryUpdate::Clear).await });
    let old = peer.received().await;
    assert!(matches!(
        topic.publish(&2, ExpiryUpdate::Clear).await,
        Err(ClientError::QueueFull)
    ));
    abandoned.abort();
    let _ = abandoned.await;
    let next = topic.clone();
    let write = tokio::spawn(async move { next.publish(&3, ExpiryUpdate::Clear).await });
    let current = peer.received().await;
    assert_ne!(old, current);
    peer.reply(old, 10).await;
    peer.reply(current, 11).await;
    assert_eq!(write.await.unwrap().unwrap().sequence, 11);
    session.close().await.unwrap();
}
#[tokio::test]
async fn lost_reply_has_unknown_outcome_and_frees_the_pending_slot() {
    let mut peer = Peer::start().await;
    let session = peer.session().await;
    let topic = session.state::<u8>("/value").unwrap();
    let sent = topic.clone();
    let write = tokio::spawn(async move { sent.publish(&1, ExpiryUpdate::Clear).await });
    let old = peer.received().await;
    assert!(matches!(
        write.await.unwrap(),
        Err(ClientError::ReplyTimeout)
    ));
    let next = topic.clone();
    let write = tokio::spawn(async move { next.publish(&2, ExpiryUpdate::Clear).await });
    let current = peer.received().await;
    peer.reply(old, 10).await;
    peer.reply(current, 11).await;
    assert_eq!(write.await.unwrap().unwrap().sequence, 11);
    session.close().await.unwrap();
}
#[tokio::test]
async fn oversized_message_fails_locally_before_transmission() {
    let peer = Peer::start().await;
    let session = peer.session().await;
    let topic = session.state::<u8>("/value").unwrap();
    assert!(matches!(
        topic
            .publish_value(Value::Bytes(vec![0; 1024 * 1024]), ExpiryUpdate::Clear)
            .await,
        Err(ClientError::Connection(ConnectionError::MessageTooLarge))
    ));
    session.close().await.unwrap();
}

#[tokio::test]
async fn uncorrelated_server_error_and_sequence_regression_terminate_all_listeners() {
    use tanuki_client::SessionEnd;
    let peer = Peer::start().await;
    let session = peer.session().await;
    let mut raw = session.listen_updates().await.unwrap();
    peer.send(ServerMessage::Error {
        request_id: None,
        error: ErrorView {
            code: "invalid_message".into(),
            message: "fixture".into(),
        },
    })
    .await;
    assert!(
        matches!(raw.recv().await, Err(SessionEnd::Remote(error)) if error.code == "invalid_message")
    );
    assert!(raw.recv().await.unwrap().is_none());
    assert!(session.close().await.is_err());
    let peer = Peer::start().await;
    let session = peer.session().await;
    let mut raw = session.listen_updates().await.unwrap();
    peer.send(ServerMessage::Update {
        sequence: 5,
        changes: vec![],
    })
    .await;
    raw.recv().await.unwrap().unwrap();
    peer.send(ServerMessage::Update {
        sequence: 5,
        changes: vec![],
    })
    .await;
    assert!(matches!(raw.recv().await, Err(SessionEnd::Sequence(_))));
    assert!(session.close().await.is_err());
}
#[tokio::test]
async fn options_reject_runtime_limits_before_opening_socket() {
    let options = SessionOptions {
        raw_capacity: std::num::NonZeroUsize::new(usize::MAX).unwrap(),
        ..Default::default()
    };
    assert!(matches!(
        Session::connect_with_options(
            "bad url",
            ClientName::parse("options").unwrap(),
            vec![],
            options
        )
        .await,
        Err(ClientError::QueueLimit { .. })
    ));
    let options = SessionOptions {
        write_timeout: std::time::Duration::MAX,
        ..Default::default()
    };
    assert!(matches!(
        Session::connect_with_options(
            "bad url",
            ClientName::parse("options").unwrap(),
            vec![],
            options
        )
        .await,
        Err(ClientError::DeadlineRange { .. })
    ));
}

#[tokio::test]
async fn remote_slow_consumer_reason_preempts_a_full_data_queue() {
    use tanuki_client::SessionEnd;
    let peer = Peer::start().await;
    let session = Session::connect_with_options(
        &peer.url,
        ClientName::parse("slow remote probe").unwrap(),
        vec![],
        SessionOptions {
            raw_capacity: std::num::NonZeroUsize::new(1).unwrap(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut full = session.listen_updates().await.unwrap();
    let mut ready = session.listen_updates().await.unwrap();
    peer.send(ServerMessage::Update {
        sequence: 1,
        changes: vec![],
    })
    .await;
    ready.recv().await.unwrap().unwrap();
    peer.replies
        .send(Message::Close(Some(
            tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: 1013.into(),
                reason: "slow_consumer".into(),
            },
        )))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), session.closed())
        .await
        .unwrap();
    assert!(
        matches!(full.recv().await,Err(SessionEnd::Connection(source)) if matches!(&*source,ConnectionError::Closed { code:1013,reason } if reason == "slow_consumer"))
    );
    assert!(full.recv().await.unwrap().is_none());
    assert!(session.close().await.is_err());
}

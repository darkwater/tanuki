//! The reconnecting `tanuki_client_core::Client` against the real server over
//! loopback WebSockets: lifecycle, correlation, setup replay, replacement and
//! resync. Server restarts reuse the same core and port.
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tanuki::{
    core::Core,
    domain::Timestamp,
    server::serve_with_core_and_timeouts,
    transport::{SharedCore, WebSocketTimeouts},
};
use tanuki_client_core::protocol::*;
use tanuki_client_core::{
    Backoff, Client, ClientBuilder, ClientError, Closed, Connection, Disconnect, Event, Events,
    SnapshotCause, Status,
};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle, time::timeout};

struct Server {
    address: SocketAddr,
    core: SharedCore,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}

impl Server {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        Self::serve(listener, Arc::new(Mutex::new(Core::new())))
    }

    fn serve(listener: TcpListener, core: SharedCore) -> Self {
        let address = listener.local_addr().unwrap();
        let (shutdown, stopped) = oneshot::channel();
        let task = tokio::spawn(serve_with_core_and_timeouts(
            listener,
            core.clone(),
            Arc::new(|| Timestamp::new(jiff::Timestamp::now())),
            WebSocketTimeouts::default(),
            async {
                let _ = stopped.await;
            },
        ));
        Self {
            address,
            core,
            shutdown,
            task,
        }
    }

    fn url(&self) -> String {
        format!("ws://{}/v1/ws", self.address)
    }

    async fn stop(self) -> (SocketAddr, SharedCore) {
        let _ = self.shutdown.send(());
        self.task.await.unwrap().unwrap();
        (self.address, self.core)
    }

    /// Stops and starts again on the same port with the same core state.
    async fn restart(self) -> Self {
        let (address, core) = self.stop().await;
        Self::serve(TcpListener::bind(address).await.unwrap(), core)
    }
}

async fn soon<T>(future: impl std::future::Future<Output = T>) -> T {
    timeout(Duration::from_secs(5), future)
        .await
        .expect("timed out")
}

fn name(name: &str) -> ClientName {
    ClientName::parse(name).unwrap()
}

fn topic(path: &str) -> TopicPath {
    TopicPath::parse(path).unwrap()
}

fn selectors(selectors: &[&str]) -> Vec<Selector> {
    selectors
        .iter()
        .map(|s| Selector::parse(s).unwrap())
        .collect()
}

fn builder(server: &Server, client: &str, selection: &[&str]) -> ClientBuilder {
    Client::builder(server.url(), name(client))
        .selectors(selectors(selection))
        .backoff(Backoff {
            initial: Duration::from_millis(20),
            max: Duration::from_millis(100),
        })
}

fn publish(path: &str, value: i64) -> Vec<WireOperation> {
    vec![WireOperation::PublishState {
        topic: topic(path),
        value: Value::Integer(value),
        expiry: None,
    }]
}

async fn next(events: &mut Events) -> Event {
    soon(events.next()).await.expect("events ended")
}

async fn connected(events: &mut Events) -> SnapshotView {
    match next(events).await {
        Event::Snapshot {
            snapshot,
            cause: SnapshotCause::Connected,
            ..
        } => snapshot,
        other => panic!("expected a connected snapshot, got {other:?}"),
    }
}

fn value_of(node: &NodeView) -> &Value {
    &node.current().expect("a current value").value
}

#[tokio::test]
async fn writes_from_cloned_handles_resolve_after_their_update_is_queued() {
    let server = Server::start().await;
    let (client, mut events) = builder(&server, "writer", &["/battery/*"]).start();
    connected(&mut events).await;
    assert_eq!(client.status(), Status::Connected);

    let clone = client.clone();
    let commit = soon(clone.write(publish("/battery/laptop", 82)))
        .await
        .unwrap();
    // The write's own update is already waiting when the write resolves.
    match next(&mut events).await {
        Event::Update(update) => assert_eq!(update.sequence, commit.sequence),
        other => panic!("expected the write's update, got {other:?}"),
    }

    let rejected = soon(client.write(vec![])).await;
    assert!(
        matches!(rejected, Err(ClientError::Rejected(ref error)) if error.code == ErrorCode::EmptyBatch),
        "{rejected:?}"
    );

    soon(client.close()).await;
    assert!(matches!(
        next(&mut events).await,
        Event::Closed(Closed::Requested)
    ));
    assert!(soon(events.next()).await.is_none());
    assert!(matches!(
        client.write(publish("/battery/laptop", 1)).await,
        Err(ClientError::Closed)
    ));
    server.stop().await;
}

#[tokio::test]
async fn select_replaces_the_selection_and_survives_reconnects() {
    let server = Server::start().await;
    let (client, mut events) = builder(&server, "selector", &["/a/*"]).start();
    connected(&mut events).await;
    client.write(publish("/b/value", 1)).await.unwrap();
    // Outside the selection: the write commits but yields no update.

    soon(client.select(selectors(&["/b/*"]))).await.unwrap();
    match next(&mut events).await {
        Event::Snapshot {
            snapshot,
            cause: SnapshotCause::Selected,
            ..
        } => assert_eq!(
            value_of(&snapshot.nodes[&topic("/b/value")]),
            &Value::Integer(1)
        ),
        other => panic!("expected the selection's snapshot, got {other:?}"),
    }

    let server = server.restart().await;
    // Shutdown usually closes with 1001 (`Ended`) but may reset the socket.
    assert!(matches!(
        next(&mut events).await,
        Event::Disconnected { .. }
    ));
    // The reconnect's hello uses the new selection.
    let snapshot = connected(&mut events).await;
    assert!(snapshot.nodes.contains_key(&topic("/b/value")));
    soon(client.close()).await;
    server.stop().await;
}

#[tokio::test]
async fn writes_fail_fast_while_disconnected_and_reconnecting_recovers() {
    let server = Server::start().await;
    let (address, core) = server.stop().await;
    // Nothing listens yet: the client keeps retrying.
    let (client, mut events) = Client::builder(format!("ws://{address}/v1/ws"), name("patient"))
        .selectors(selectors(&["/p/*"]))
        .backoff(Backoff {
            initial: Duration::from_millis(20),
            max: Duration::from_millis(50),
        })
        .start();
    assert!(matches!(
        next(&mut events).await,
        Event::Disconnected {
            error: Disconnect::Connection(_),
            ..
        }
    ));
    assert!(matches!(
        client.write(publish("/p/value", 1)).await,
        Err(ClientError::NotConnected)
    ));

    let server = Server::serve(TcpListener::bind(address).await.unwrap(), core);
    loop {
        match next(&mut events).await {
            Event::Disconnected { .. } => {}
            Event::Snapshot {
                cause: SnapshotCause::Connected,
                ..
            } => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    soon(client.write(publish("/p/value", 2))).await.unwrap();
    soon(client.close()).await;
    server.stop().await;
}

#[tokio::test]
async fn the_setup_batch_reclaims_inputs_before_other_writes_after_a_reconnect() {
    let server = Server::start().await;
    let lamp = topic("/lamp/desired");
    let setup = vec![
        WireOperation::DefineInput {
            topic: lamp.clone(),
            kind: WireInputKind::Desired,
        },
        WireOperation::ClaimInput {
            topic: lamp.clone(),
            release: WireRelease::Immediate,
        },
    ];
    let (client, mut events) = builder(&server, "lamp", &["/lamp/**"]).setup(setup).start();
    connected(&mut events).await;
    let claim_owner = |event: Event| match event {
        Event::Update(update) => update.changes.into_iter().find_map(|change| match change {
            ChangeView::Upsert {
                node: NodeView::Desired { claim, .. },
                ..
            } => Some(claim.map(|claim| claim.owner)),
            _ => None,
        }),
        other => panic!("expected an update, got {other:?}"),
    };
    assert_eq!(
        claim_owner(next(&mut events).await),
        Some(Some(name("lamp")))
    );

    // A restart ends the session, which releases the claim immediately.
    let server = server.restart().await;
    assert!(matches!(
        next(&mut events).await,
        Event::Disconnected { .. }
    ));
    let snapshot = connected(&mut events).await;
    assert!(matches!(
        &snapshot.nodes[&lamp],
        NodeView::Desired { claim: None, .. }
    ));
    // Issued straight after the snapshot, this write still commits after the
    // setup batch: the re-claim's update comes first.
    client
        .write(vec![WireOperation::SubmitDesired {
            topic: lamp.clone(),
            value: Value::Integer(70),
            expiry: None,
        }])
        .await
        .unwrap();
    assert_eq!(
        claim_owner(next(&mut events).await),
        Some(Some(name("lamp")))
    );
    assert!(matches!(next(&mut events).await, Event::Update(_)));
    soon(client.close()).await;
    server.stop().await;
}

#[tokio::test]
async fn a_rejected_setup_is_reported_and_the_connection_stays_usable() {
    let server = Server::start().await;
    let (client, mut events) = builder(&server, "bad-setup", &["/s/*"])
        .stateless()
        .setup(vec![WireOperation::ClaimInput {
            topic: topic("/s/input"),
            release: WireRelease::Immediate,
        }])
        .start();
    connected(&mut events).await;
    assert!(matches!(
        next(&mut events).await,
        Event::SetupRejected(ClientError::Rejected(error)) if error.code == ErrorCode::SessionRequired
    ));
    soon(client.write(publish("/s/value", 1))).await.unwrap();
    soon(client.close()).await;
    server.stop().await;
}

#[tokio::test]
async fn a_replaced_session_closes_without_reconnecting() {
    let server = Server::start().await;
    let (client, mut events) = builder(&server, "twin", &[]).start();
    connected(&mut events).await;

    let mut newer = Connection::open(&server.url()).await.unwrap();
    newer
        .send(&ClientMessage::Hello {
            request_id: RequestId::new("h".into()),
            client: name("twin"),
            selectors: vec![],
            session: SessionMode::Managed,
        })
        .await
        .unwrap();
    assert!(matches!(
        soon(newer.recv()).await.unwrap(),
        Some(ServerMessage::Snapshot { .. })
    ));

    assert!(matches!(
        next(&mut events).await,
        Event::Closed(Closed::Replaced)
    ));
    assert!(soon(events.next()).await.is_none());
    assert_eq!(client.status(), Status::Closed);
    assert_eq!(server.core.lock().unwrap().managed_session_count(), 1);
    newer.close().await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn a_consumer_that_falls_behind_gets_a_resync_snapshot() {
    let server = Server::start().await;
    let (client, mut events) = builder(&server, "slow", &["/r/*"])
        .event_capacity(2)
        .start();
    connected(&mut events).await;
    for value in 0..10 {
        client.write(publish("/r/value", value)).await.unwrap();
    }
    // Fold events like a consumer would. Overflow may happen several times;
    // what matters is that the folded state converges on the last write.
    let mut view = SelectedView::from_snapshot(SnapshotView {
        sequence: 0,
        nodes: Default::default(),
    });
    let mut resyncs = 0;
    while view
        .nodes()
        .get(&topic("/r/value"))
        .is_none_or(|node| value_of(node) != &Value::Integer(9))
    {
        match next(&mut events).await {
            Event::Update(update) => {
                view.apply(update).unwrap();
            }
            Event::Snapshot {
                snapshot,
                cause: SnapshotCause::Resync,
                ..
            } => {
                resyncs += 1;
                view = SelectedView::from_snapshot(snapshot);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(resyncs > 0);
    soon(client.close()).await;
    server.stop().await;
}

#[tokio::test]
async fn a_silent_server_times_out_the_connection_attempt() {
    // Accepts TCP but never completes the WebSocket handshake.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (client, mut events) = Client::builder(format!("ws://{address}/v1/ws"), name("waiting"))
        .connect_timeout(Duration::from_millis(100))
        .start();
    assert!(matches!(
        next(&mut events).await,
        Event::Disconnected {
            error: Disconnect::Timeout,
            ..
        }
    ));
    assert!(matches!(client.status(), Status::Reconnecting { .. }));
    // Closing works while waiting to retry.
    soon(client.close()).await;
    assert!(matches!(
        next(&mut events).await,
        Event::Closed(Closed::Requested)
    ));
    drop(listener);
}

#[tokio::test]
async fn dropping_every_handle_closes_the_connection() {
    let server = Server::start().await;
    let (client, mut events) = builder(&server, "dropped", &[]).start();
    connected(&mut events).await;
    assert_eq!(server.core.lock().unwrap().managed_session_count(), 1);
    drop(client);
    assert!(matches!(
        next(&mut events).await,
        Event::Closed(Closed::Requested)
    ));
    soon(async {
        while server.core.lock().unwrap().managed_session_count() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    server.stop().await;
}

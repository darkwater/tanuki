# tanuki-client-core

The connection layer of the Tanuki Rust client, on native Tokio and browser
WebAssembly. It deals with everything about staying connected and leaves state
management to the crate above it ([`tanuki-client`](../tanuki-client)):

- [`Client`]: a cheap, cloneable handle for writing, plus an [`Events`]
  stream of snapshots and updates. A background task connects, says hello,
  reconnects with backoff, correlates replies and re-sends the selection and a
  setup batch after every reconnect. Request IDs never leave this crate.
- [`Connection`]: one raw WebSocket exchanging typed protocol messages, with
  nothing automated. `Client` is built on it.

```rust,no_run,standalone_crate
use tanuki_client_core::{Client, Event, protocol::*};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let (client, mut events) = Client::builder("ws://127.0.0.1:5167/v1/ws", ClientName::parse("laptop")?)
    .selectors([Selector::parse("/battery/*")?])
    .start();

// Clone `client` freely; each clone writes through the same connection.
let writer = client.clone();
writer
    .write(vec![WireOperation::PublishState {
        topic: TopicPath::parse("/battery/laptop")?,
        value: Value::Integer(82),
        expiry: None,
    }])
    .await?;

while let Some(event) = events.next().await {
    match event {
        Event::Snapshot { snapshot, cause, .. } => println!("{cause:?}: {} nodes", snapshot.nodes.len()),
        Event::Update(update) => println!("{}: {:?}", update.sequence, update.changes),
        Event::SetupRejected(error) => eprintln!("setup failed: {error}"),
        Event::Disconnected { error, retry_in } => eprintln!("{error}; retrying in {retry_in:?}"),
        Event::Closed(reason) => println!("closed: {reason:?}"),
    }
}
# Ok(())
# }
```

## The client

**Events.** A hello is always answered by exactly one snapshot, so every
connection begins with `Snapshot { cause: Connected }`: replace whatever you
derived before it. Each `Update` is one complete commit. `Disconnected` means
derived state is stale until the next snapshot. `Closed` is always the last
event.

**Writes.** [`Client::write`] sends one atomic batch and waits for its reply
(default timeout 10 s). The server sends a write's own update before its
reply, so when `write` returns `Ok` the update is already queued on `Events`.
While disconnected, writes fail at once with
[`ClientError::NotConnected`]; nothing is queued across reconnects. A write
whose connection drops before its reply fails with [`ClientError::Lost`]: it
may or may not have committed.

**Selection.** [`ClientBuilder::selectors`] sets the initial selection and
[`Client::select`] replaces it (`Snapshot { cause: Selected }`). Every
reconnect's hello uses the latest selection.

**Setup batch.** Claims belong to a managed session and are lost when it ends.
[`ClientBuilder::setup`] (or [`Client::set_setup`] later) holds operations,
typically `define_input` and `claim_input`, that are written as one batch right
after every hello. Because the server handles one socket's messages in order,
setup commits before any write sent after it. If the server rejects it, you get
`Event::SetupRejected` and the connection stays up.

**Falling behind.** The driver never waits for the `Events` reader, because it
also answers pings and resolves writes. When more than
[`ClientBuilder::event_capacity`] updates (default 1024) are queued, the queued
updates are dropped and the client requests a fresh snapshot of the same
selection (`Snapshot { cause: Resync }`). Event and command occurrences in the
dropped updates are lost. Lifecycle events are never dropped.

**Reconnecting and closing.** Failed or lost connections are retried with
exponential backoff (0.5 s doubling to 30 s, with jitter; see [`Backoff`]). The
connect timeout covers opening the socket and receiving the hello's snapshot
(default 10 s). The client stops without retrying when:

- [`Client::close`] is called, or every `Client` is dropped
  (`Closed::Requested`);
- a newer managed connection takes the client name (close code 4001,
  `Closed::Replaced`). Retrying would take the name back;
- the server rejects the hello (`Closed::Rejected`).

Natively, [`ClientBuilder::start`] spawns a Tokio task, so call it inside a
Tokio runtime. In the browser it uses `spawn_local` and browser timers, with no
Tokio runtime.

## The raw connection

```rust,no_run,standalone_crate
use tanuki_client_core::{Connection, protocol::*};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let mut connection = Connection::open("ws://127.0.0.1:5167/v1/ws").await?;
connection
    .send(&ClientMessage::Hello {
        request_id: RequestId::new("hello".into()),
        client: ClientName::parse("laptop")?,
        selectors: vec![Selector::parse("/battery/*")?],
        session: SessionMode::Managed,
    })
    .await?;
while let Some(message) = connection.recv().await? {
    match message {
        ServerMessage::Snapshot { sequence, nodes, .. } => println!("{sequence}: {} nodes", nodes.len()),
        ServerMessage::Update { sequence, changes } => println!("{sequence}: {changes:?}"),
        ServerMessage::Reply { request_id, result } => println!("{request_id:?} committed as {}", result.sequence),
        ServerMessage::Error { request_id, error } => eprintln!("{request_id:?}: {error:?}"),
    }
}
# Ok(())
# }
```

### What the connection does

- Opens `ws://` (and natively `wss://`, through the default `rustls` feature
  with webpki roots) connections to `/v1/ws`.
- Encodes [`ClientMessage`](protocol::ClientMessage)s as JSON text or
  MessagePack binary ([`Codec`]); [`Connection::send_with_codec`] picks per
  message, and the server replies in the codec of the request's frame.
- Decodes every incoming frame by its type into a
  [`ServerMessage`](protocol::ServerMessage), rejecting trailing data.
- Enforces the 1 MiB message limit: an oversized outgoing message fails with
  [`ConnectionError::MessageTooLarge`] before anything is sent.
- Answers WebSocket pings while you keep calling `recv` (browsers do this
  themselves).
- Reports a normal close (1000/1001) as `recv` returning `None`, and any other
  close as [`ConnectionError::Closed`] with its code and stable reason string,
  such as `4001 session_replaced`.

### What it leaves to you

Request IDs, matching replies to requests, caching snapshots and applying
updates, reconnecting, timeouts, re-sending hello and re-claiming inputs after a
reconnect: [`Client`] does all of these except caching. The protocol guarantees that make this tractable are in
[`docs/protocol.md`](../docs/protocol.md): a hello is answered by exactly one
snapshot before anything else, every update is one complete commit, and a
write's own update arrives before its reply.

`tanuki_client_core::protocol` (the `tanuki-protocol` crate) has the message types,
the [`Value`](protocol::Value) runtime value, and
[`SelectedView`](protocol::SelectedView), a minimal cache that applies
snapshots and updates for one selection. Payloads are plain Serde:
[`to_value`](protocol::to_value) and [`from_value`](protocol::from_value)
convert any `Serialize`/`Deserialize` type, and `Value` is itself an ordinary
Serde type, so it can appear inside your own payload structs. `Vec<u8>`
becomes bytes; fixed arrays such as `[u8; 3]` stay lists.

### Sending and receiving concurrently

`send` takes `&self` and `recv` takes `&mut self`. To receive in one task while
sending from others, split the connection; the [`Sender`] is cheap to clone:

```rust,no_run,standalone_crate
use tanuki_client_core::{Connection, protocol::*};

# async fn example(connection: Connection) -> Result<(), tanuki_client_core::ConnectionError> {
let (sender, mut receiver) = connection.split();
let receiving = async move {
    while let Ok(Some(message)) = receiver.recv().await {
        // apply snapshots and updates, resolve replies...
    }
};
// Spawn `receiving` on your executor (tokio::spawn, spawn_local, ...).
sender.send(&ClientMessage::Write { request_id: RequestId::new("w1".into()), operations: vec![] }).await?;
# Ok(())
# }
```

Keep a receiver running for as long as the connection should live: the server
closes a connection it has not heard from (not even a pong) for 60 seconds.

## Browser WebAssembly

The same API works on `wasm32-unknown-unknown` with the browser's WebSocket and
no Tokio runtime; drive futures with your framework's executor or
`wasm_bindgen_futures::spawn_local`. Browsers manage `wss://` themselves.
Browser network buffers sit outside this crate's limits, and the 1 MiB receive
check happens after the browser has received a message.

Run the browser tests against an isolated server (needs Chrome or Chromium,
a matching chromedriver, and `wasm-bindgen-cli` matching the lockfile):

```sh
cargo install wasm-bindgen-cli --version 0.2.130 --locked
python3 tanuki-client-core/tests/run-browser.py
```

# tanuki-client

The Tanuki WebSocket protocol for Rust, on native Tokio and browser
WebAssembly: open a connection, send typed client messages, receive typed
server messages. Build state management on top of it.

```rust,no_run
use tanuki_client::{Connection, protocol::*};

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

## What this crate does

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

## What it leaves to you

Request IDs, matching replies to requests, caching snapshots and applying
updates, reconnecting, timeouts, re-sending hello and re-claiming inputs after a
reconnect. The protocol guarantees that make this tractable are in
[`docs/protocol.md`](../docs/protocol.md): a hello is answered by exactly one
snapshot before anything else, every update is one complete commit, and a
write's own update arrives before its reply.

`tanuki_client::protocol` (the `tanuki-protocol` crate) has the message types,
the [`Value`](protocol::Value) runtime value with
[`to_value`](protocol::to_value)/[`from_value`](protocol::from_value) for Serde
types, and [`SelectedView`](protocol::SelectedView), a minimal cache that
applies snapshots and updates for one selection.

## Sending and receiving concurrently

`send` takes `&self` and `recv` takes `&mut self`. To receive in one task while
sending from others, split the connection; the [`Sender`] is cheap to clone:

```rust,no_run
use tanuki_client::{Connection, protocol::*};

# async fn example(connection: Connection) -> Result<(), tanuki_client::ConnectionError> {
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
python3 tanuki-client/tests/run-browser.py
```

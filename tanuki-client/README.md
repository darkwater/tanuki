# tanuki-client

Async Rust SDK for Tanuki, for native Tokio applications and browser WebAssembly.

```rust,no_run
use tanuki_client::{Session, StateTopic};

# async fn example() -> Result<(), tanuki_client::ClientError> {
let session = Session::connect("ws://127.0.0.1:5167/v1/ws", "laptop").await?;
let battery = StateTopic::<u8>::new("/battery/laptop")?;
session.publish(&battery, &82).await?;

let mut batteries = session.observe("/battery/*")?;
while let Some(snapshot) = batteries.recv().await {
    for (path, level) in snapshot.values::<u8>() {
        println!("{path}: {level:?}");
    }
}
# Ok(())
# }
```

## Concepts

- A **[`Session`]** is one connection to a server. It is cheap to clone and
  share; all clones use the same connection. It reconnects with backoff after a
  disconnect (see [`Reconnect`]), and keeps every observer, listener and
  controller working across reconnects. It ends when you call
  [`Session::close`], when the last clone is dropped, or when another managed
  connection with the same client name replaces it.
- A **[`Topic`]** handle names one topic and its payload type: `StateTopic<T>`,
  `EventTopic<T>`, `DesiredTopic<T>` and `CommandTopic<T>`. Handles are plain
  values, independent of any session; create them anywhere. Payloads are any
  Serde type, or [`Value`] for arbitrary data. Use `serde_bytes` for byte
  fields; a plain `Vec<u8>` is a list. Dates and intervals are ordinary strings
  or integers (for example `jiff::Timestamp`, which serializes as RFC 3339).
- The client name attributes your writes. A **managed** session (the default)
  can claim inputs, and a newer managed connection with the same name replaces
  it. A **stateless** session ([`SessionBuilder::stateless`]) cannot claim, and
  never replaces or is replaced: use it for dashboards, inspectors and browser
  tabs that may run several times under one name.

## Writing

```rust,no_run
use std::time::Duration;
use tanuki_client::{EventTopic, Expiry, Session, StateTopic};

# async fn example(session: Session) -> Result<(), tanuki_client::ClientError> {
let hue = StateTopic::<u16>::new("/lamp/hue")?;
let brightness = StateTopic::<u8>::new("/lamp/brightness")?;
let doorbell = EventTopic::<String>::new("/doorbell/rang")?;

session.publish(&hue, &120).await?; // stays until replaced
session
    .publish_with(&brightness, &70, Expiry::After(Duration::from_secs(90))) // a lease
    .await?;
session.emit(&doorbell, &"front".to_owned()).await?;

// An atomic batch: observers see both values in one snapshot, or neither.
let receipt = session.batch().publish(&hue, &30).publish(&brightness, &40).commit().await?;
println!("committed as {}; warnings: {:?}", receipt.sequence, receipt.warnings);
# Ok(())
# }
```

A receipt means the server accepted the write; it does not promise that a
device acted. When it arrives, your own observers already reflect the write.
Errors tell you whether a write might have been applied:
`Disconnected`, `Busy`, `Payload` and `Rejected` mean it was not;
`OutcomeUnknown` and `ReplyTimeout` mean it may have been. Writes are never
retried for you.

## Reading current state

[`Session::observe`] accepts selector text (`"/battery/*"`, `"/lamp/**"`,
`"/tv/{volume,mute}"`), topic handles, or several of them as an array, tuple or
`Vec`. The session widens its server subscription as needed.

```rust,no_run
use tanuki_client::{Session, StateTopic};

# async fn example(session: Session) -> Result<(), tanuki_client::ClientError> {
let hue = StateTopic::<u16>::new("/lamp/hue")?;
let brightness = StateTopic::<u8>::new("/lamp/brightness")?;
let mut lamp = session.observe((&hue, &brightness))?;
while let Some(snapshot) = lamp.recv().await {
    // Ok(None): no value right now. Err: wrong kind, or the payload did not decode.
    println!("hue {:?}, brightness {:?}", snapshot.get(&hue), snapshot.get(&brightness));
}
# Ok(())
# }
```

Each [`Snapshot`] is a complete, coherent view of the selected topics after
one server commit; it never changes afterwards. The first `recv` returns once
the server has sent the data; later calls wait for a commit that changes the
selection. A slow reader skips intermediate snapshots. `snapshot.read(&topic)`
returns the full [`Node`] with provenance, expiry and claim, and
`snapshot.iter()` gives raw [`NodeView`]s.

To see every change, including event and command occurrences, use
[`Session::listen`] for raw [`Event`]s, or [`Session::occurrences`] for typed
occurrences of one topic. A listener that falls too far behind receives a fresh
[`Snapshot`] with [`Resync::Lagged`] instead of the backlog; after a reconnect
it receives one with [`Resync::Reconnected`]. Occurrences during either gap
are lost, never replayed.

## Controlling an input

A controller owns an input: clients submit requests, it acts on them and
publishes the actual state separately.

```rust,no_run
use tanuki_client::{ClaimRelease, DesiredTopic, Request, Session, StateTopic};

# async fn example(session: Session) -> Result<(), tanuki_client::ClientError> {
let desired = DesiredTopic::<u8>::new("/lamp/desired-brightness")?;
let actual = StateTopic::<u8>::new("/lamp/brightness")?;
let mut requests = session.control(&desired, ClaimRelease::Immediate).await?;
while let Some(request) = requests.recv().await {
    match request {
        Ok(Request::Submitted(request)) => {
            // ...drive the device, then report what it actually did:
            session.publish(&actual, &request.value).await?;
        }
        Ok(Request::Cleared) => {}
        Err(error) => eprintln!("{error}"), // a bad payload, or the claim was lost
    }
}
# Ok(())
# }
```

[`Session::control`] defines and claims the input (taking over another
session's claim), reports a request that was already pending, and then each
new submission once, even when a value repeats. After a reconnect the session
re-claims the input, unless another session claimed it meanwhile; then the
controller stops with a claim error instead of fighting over it. Command
inputs work the same way and yield typed [`Occurrence`]s. Anyone may submit:
`session.submit(&desired, &70)`.

## Connection status

[`SessionBuilder::connect`] fails if the first attempt does;
[`SessionBuilder::start`] returns at once and keeps trying, which suits UIs.
Follow [`Session::status`] to show connectivity. While disconnected, writes
fail with `Disconnected` and observers keep their last snapshot until fresh
data arrives.

```rust,no_run
use std::time::Duration;
use tanuki_client::{Codec, Reconnect, Session, Status};

# async fn example() -> Result<(), tanuki_client::ClientError> {
let session = Session::builder("wss://tanuki.example/v1/ws", "kitchen tablet")
    .stateless()
    .codec(Codec::MessagePack)
    .reconnect(Reconnect::Backoff { initial: Duration::from_secs(1), max: Duration::from_secs(30) })
    .start()?;
let mut status = session.status();
while let Some(status) = status.changed().await {
    if !matches!(status, Status::Connected) {
        println!("showing stale data: {status:?}");
    }
}
# Ok(())
# }
```

Native builds support `ws://` and, through the default `rustls` feature,
`wss://` with webpki roots. The ring provider is installed unless your
application installed a rustls `CryptoProvider` first; disable default
features to drop TLS.

## Lower levels

[`Connection`] is direct message access without hello automation, caches or
background tasks: send `protocol::ClientMessage`s with your own request IDs and
receive every `protocol::ServerMessage`. Each WebSocket message carries one
JSON (text) or MessagePack (binary) protocol message of at most 1 MiB.
[`Session::write`] sends raw operations through a session.

## Browser WebAssembly

The same API works on `wasm32-unknown-unknown`, using the browser's WebSocket
(`ws://` or browser-managed `wss://`) and timers. No Tokio runtime or feature
switch is needed; run futures on your framework's executor or
`wasm_bindgen_futures::spawn_local`.

```rust,no_run
# #[cfg(target_arch = "wasm32")]
# mod browser_example {
use tanuki_client::{Session, StateTopic};

pub fn start(url: String) {
    wasm_bindgen_futures::spawn_local(async move {
        let session = Session::builder(&url, "web dashboard").stateless().start().unwrap();
        let battery = StateTopic::<u8>::new("/battery/laptop").unwrap();
        let mut view = session.observe(&battery).unwrap();
        while let Some(snapshot) = view.recv().await {
            tracing::info!(level = ?snapshot.get(&battery), "battery");
        }
    });
}
# }
```

Browsers throttle timers in background tabs, so deadlines and reconnects may
fire late. Browser network buffers sit outside the SDK's queue limits, and the
1 MiB receive limit applies after the browser has received a message. Closing
or unloading a page cannot guarantee a clean WebSocket close.

Run the browser tests against an isolated server (needs Chrome or Chromium,
a matching chromedriver, and `wasm-bindgen-cli` matching the lockfile):

```sh
cargo install wasm-bindgen-cli --version 0.2.130 --locked
python3 tanuki-client/tests/run-browser.py
```

## Examples

With a server running (`cargo run -p tanuki`), in separate terminals:

```sh
cargo run -p tanuki-client --example dashboard   # batteries and lamp, survives restarts
cargo run -p tanuki-client --example controller  # a simulated lamp controller
cargo run -p tanuki-client --example publisher   # publishes a battery lease
curl -H 'Tanuki-Client: remote' -H 'Content-Type: application/json' \
  -d '{"operations":[{"op":"submit_desired","topic":"/lamp/desired-brightness","value":70}]}' \
  http://127.0.0.1:5167/v1/write
```

Each example takes the WebSocket URL as its first argument.

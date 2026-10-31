# tanuki-client

Async Rust SDK for Tanuki's v1 WebSocket protocol, on native and browser WASM. Use the repository's
pinned nightly. The client depends on `tanuki-protocol`, never on the server.
Native uses Tokio/Tungstenite with `ws://`. Browser builds for
`wasm32-unknown-unknown` use the browser's WebSocket API (`ws://`/`wss://`), local
tasks and browser timers. The same Rust Session, typed handles and observers
work on both targets. JavaScript bindings and framework integration remain deferred.

## Start with a session

```rust,no_run,standalone_crate
use tanuki_client::{Session, ExpiryUpdate};
use tanuki_client::protocol::{ClientName, Selector};

# async fn example() -> Result<(), tanuki_client::ClientError> {
let session = Session::connect(
    "ws://127.0.0.1:5167/v1/ws",
    ClientName::parse("lamp dashboard").unwrap(),
    vec![Selector::parse("/lamp/**").unwrap()],
).await?;
let hue = session.state::<u16>("/lamp/hue")?;
let brightness = session.state::<u8>("/lamp/brightness")?;
let mut observer = session.observe_batch([
    hue.path().clone(), brightness.path().clone(),
]).await?;

let mut batch = session.batch();
batch.publish(&hue, &120, ExpiryUpdate::Clear)?;
batch.publish(&brightness, &70, ExpiryUpdate::Clear)?;
let receipt = batch.commit().await?;
println!("accepted {}, warnings: {:?}", receipt.sequence, receipt.warnings);
let initial = observer.recv().await?; // Registration baseline, even if empty.
let current = observer.recv().await?; // Latest complete unseen selected snapshot.
println!("initial: {initial:?}, current: {current:?}");
session.close().await?;
# Ok(())
# }
```

Handles are local, cheap to clone and hold weak session references. Creating one
does not define or claim a node. `desired::<T>` and `command::<T>` expose separate
`define`, `claim` and `submit` operations. A submission requires an existing
input definition; batch definition and submission when creating pending intent.
Dropping an uncommitted batch sends nothing. Bound handles from another session
are rejected by a batch. `submit(&None, ...)` is a null payload; desired `clear`
is an explicit operation.

`SessionOptions` selects JSON or MessagePack outgoing frames, connect/hello and
write deadlines (10 seconds by default), pending request capacity (64) and raw
queue capacity per listener (64). Incoming frames determine their own decoder.
Hello warnings are available through `hello_warnings`; `initial_snapshot` is the
historical hello baseline. `termination`/`closed` expose session termination.
No idle timeout, reconnect, retry or automatic claim recovery is performed.

## Read complete state or every delta

`observe(&handle)` selects one exact path. `observe_batch` deduplicates exact
paths and checks membership in the immutable hello selection. A covered absent
topic is valid; an uncovered topic gives `NotSubscribed`. Snapshots expose
`sequence`, `paths`, `nodes`, raw `node` metadata and fallible typed `read`.
`read` distinguishes an absent selected node, a desired node with no current
payload, explicit null, wrong kind and invalid payload. Reading a path omitted
from this observer gives `ReadError::NotObserved`. Decode failure leaves the raw
snapshot intact and does not end the observer.

The registration baseline is delivered once, even if later updates are already
available. Subsequent output coalesces to the latest complete snapshot; projection
tasks apply every input delta. Earlier returned snapshots remain immutable.
Metadata-only changes and matching occurrences can advance observation output;
occurrence payloads are available only on raw listeners. Instant node definitions
and publisher/claim metadata can appear in snapshots, without retained payloads.

`listen_updates().await` captures its retained baseline and installs delivery at
one serialized driver boundary. Inspect `initial_snapshot` and then call `recv`
for each complete batch. Raw queues are bounded: loss ends only that listener
with `SessionEnd::Lagged`, reported once, followed by `None`. An observer whose
raw input lags also ends; registering again recovers current retained data but
cannot recover occurrences. Abnormal termination has an independent notification
path and preempts queued data. Clean closure drains admitted raw batches and
allows one final unseen latest snapshot. Both raw listeners and observers offer
`into_stream`; pin the returned standard Stream before polling it.

Do not treat sequence gaps as loss, or a receipt sequence as observer progress.
Filtered global commits can create legitimate gaps. A write reply means core
acceptance, without promising execution, persistence or observed UI state.

## Payloads and wire access

`to_value`/`from_value` use `serde-value`, independently of wire tags. Structs and
maps occupy one topic. Signed integers and fitting unsigned integers retain
exact values through 64 bits. Overflow, nonfinite floats and non-string map keys
fail locally. Use `serde_bytes` on byte fields; ordinary `Vec<u8>` is a list.
Tag-looking map keys remain literal data. The current bridge does not support
Serde's 128-bit integer methods; narrow explicitly to the supported i64 domain.
Semantic timestamps/durations remain accessible as explicit `Value` variants;
arbitrary derived time fields are not automatically interpreted as semantic
values. Use handle `publish_value`/`submit_value`/`emit_value` or raw operations.
Server schema validation remains authoritative.

`Connection::open` provides direct send/receive without a cache, hello automation
or background tasks. Send `protocol::ClientMessage` with caller-chosen request IDs;
receive every `ServerMessage` explicitly. `send_with_codec` supports mixed codec
frames. Abnormal close codes and transport/codec errors preserve their sources;
normal close ends receiving. Per-wire-message limit is 1 MiB.
Each data message must contain exactly one JSON or MessagePack protocol message;
trailing bytes are a decode error. `protocol::decode_messagepack` provides the
same framing check for code using the DTOs with its own transport.

`QueueFull`, payload/encoding failures and `UnsentTimeout` indicate local failure
before transmission. `SessionGone` also means the write was not handed to the
socket writer, including queued requests rejected during shutdown.
`ReplyTimeout` and `OutcomeUnknown` mean a write may have
been applied. Dropping a write future does not undo a mutation. Canceled waiters
release capacity, and late replies cannot resolve another request.

## Lifetime and examples

Session owns the driver/writer and projection tasks. Dropping an observer
cancels only its projection. Dropping Session signals shutdown; weak topic handles
cannot keep it alive. `close().await` joins tasks, attempts WebSocket close and
bounds writer shutdown to one second; a prior abnormal termination is returned.
There is no library-owned runtime or blocking destructor.

Shutdown rejects requests still in the driver's command queue. Requests already
handed to the socket writer may still be applied; closing is not rollback.

## Browser WebAssembly

Install the target on the pinned compiler, then check the SDK:

```sh
rustup target add wasm32-unknown-unknown
cargo check -p tanuki-client --target wasm32-unknown-unknown --locked
```

Depend on `tanuki-client` normally from a Rust web application. Target-specific
dependencies select the browser adapter automatically; no SDK feature switch or
Tokio runtime is needed. Run application futures on your framework's local
executor or `wasm_bindgen_futures::spawn_local` (add `wasm-bindgen-futures` to
your application dependencies when using it directly). For example:

```rust,no_run,standalone_crate
# #[cfg(target_arch = "wasm32")]
# mod browser_example {
use tanuki_client::{ClientError, Session};
use tanuki_client::protocol::{ClientName, Selector};

pub fn start(url: String) {
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = watch_battery(&url).await {
            tracing::error!(%error, "battery subscription ended");
        }
    });
}

async fn watch_battery(url: &str) -> Result<(), ClientError> {
    let session = Session::connect(
        url, ClientName::parse("web dashboard").unwrap(),
        vec![Selector::parse("/battery/*").unwrap()],
    ).await?;
    let battery = session.state::<u8>("/battery/laptop")?;
    let mut observer = session.observe(&battery).await?;
    while let Some(snapshot) = observer.recv().await? {
        // Pass snapshot.read(&battery) to your UI here.
        tracing::debug!(sequence = snapshot.sequence(), "battery view updated");
    }
    session.close().await
}
# }
```

Use `wss://` behind a TLS proxy for an HTTPS page. Certificate validation,
mixed-content rules, connection failures and ping/pong handling belong to the
browser. Browser errors may hide handshake details. The server still accepts
the same `/v1/ws` protocol and validates every write.

The browser may throttle timers in background tabs or suspend tasks entirely;
deadlines fire when its event loop runs, without retrying writes. The SDK keeps
its bounded pending/write/raw queues and latest-state observer policy. Browser
network buffers and Gloo's incoming event queue are additional buffers outside
those limits. The 1 MiB receive limit is checked after the browser has assembled
and allocated a complete message; it is not a total browser memory limit.

`close().await` joins SDK tasks and requests a WebSocket close. It does not wait
for a server cleanup acknowledgement. Dropping Session signals shutdown while
the event loop runs; unloading a page cannot guarantee awaited cleanup.
Native examples below need Tokio. Node, WASI, web workers, Firefox/Safari and a
deployed browser application have not been verified by this increment.

Run browser acceptance tests against an isolated production server with:

```sh
cargo install wasm-bindgen-cli --version 0.2.130 --locked
# Install Chrome + matching chromedriver (or configure another WebDriver).
python3 tanuki-client/tests/run-browser.py
python3 tanuki-client/tests/run-browser.py --release
```

The helper uses temporary storage and dynamically assigned ports, then closes
the server. `CHROMEDRIVER` selects the driver binary; optional `CHROME_BIN`
selects a nonstandard browser binary. The test-runner version must match the
lockfile's wasm-bindgen version. See [testing](../docs/testing.md) for coverage.

## Native examples

From the repository root, run the server and use separate terminals:

```sh
cargo run -p tanuki --bin tanuki
cargo run -p tanuki-client --example dashboard
cargo run -p tanuki-client --example publisher
cargo run -p tanuki-client --example controller
```

Each example accepts the WebSocket URL as its first argument. The publisher
writes `/battery/laptop = 82` with a 120-second expiry. The controller explicitly
defines/claims `/lamp/desired-brightness` and simulates actual output as one
`/lamp/hue` + `/lamp/brightness` batch. Send intent using the existing HTTP API:

```sh
curl -H 'Tanuki-Client: example remote' -H 'Content-Type: application/json' \
  -d '{"operations":[{"op":"submit_desired","topic":"/lamp/desired-brightness","value":70,"expiry":{"mode":"clear"}}]}' \
  http://127.0.0.1:5167/v1/write
```

Controller and dashboard stop with Ctrl-C. These examples were compiled and
exercised together on loopback against the production binary. They are simulated
clients, without verified hardware, deployed dashboards or Iced integration.

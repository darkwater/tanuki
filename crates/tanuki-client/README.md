# tanuki-client

Native async Rust SDK for Tanuki's v1 WebSocket protocol. Use the repository's
pinned nightly. The client depends on `tanuki-protocol`, never on the server.
The current native transport profile is `ws://` with Tokio/Tungstenite; TLS,
browser transport, JavaScript bindings and framework integration are deferred.

## Start with a session

```rust,no_run
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

`QueueFull`, payload/encoding failures and `UnsentTimeout` indicate local failure
before transmission. `ReplyTimeout` and `OutcomeUnknown` mean a write may have
been applied. Dropping a write future does not undo a mutation. Canceled waiters
release capacity, and late replies cannot resolve another request.

## Lifetime and examples

Session owns the native driver/writer and projection tasks. Dropping an observer
cancels only its projection. Dropping Session signals shutdown; weak topic handles
cannot keep it alive. `close().await` joins tasks, attempts WebSocket close and
bounds writer shutdown to one second; a prior abnormal termination is returned.
There is no library-owned runtime or blocking destructor.

From the repository root, run the server and use separate terminals:

```sh
cargo run --bin tanuki
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

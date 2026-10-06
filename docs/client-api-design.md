# Client API crate proposal

2026-10-06. Design and handoff only; no SDK implementation has started.
Start with [client-api-handoff.md](client-api-handoff.md) when resuming work.
The user selected WebSocket-only scope, a protocol-oriented low-level API,
and convenience helpers built on its primitives. A background reader fans out
updates through channels; `session.observe(&topic)` and batch observation build
current-state views from task-local caches. Serde shapes across topics can come
later. Typed payloads and Rust delivery first with browser WASM accounted for
now remain the direction. The user selected latest-complete-state delivery for
snapshot observers: intermediate snapshots may be skipped by a slow consumer.
Raw update listeners preserve batches and report lag explicitly.
Iced is the primary consumer use case. Keep the SDK framework-independent and
avoid overfitting its API to Iced; no Iced dependency is required.
The remaining choices below are recommendations for review, not accepted
product requirements. API sketches are illustrative and have not been compiled.

## Scope and package boundary

Start with two reusable crates in this repository's Cargo workspace:

| Package | Responsibility |
| --- | --- |
| `tanuki-protocol` | Shared value/path/selector primitives, wire requests and responses, JSON/MessagePack codecs |
| `tanuki-client` | WebSocket primitives, managed session driver, channel-based observations and typed payload helpers |
| Existing `tanuki` | Server, authoritative domain transitions, schema/link policy, persistence |

Both server and client depend on protocol. Protocol depends on neither.
Consumers normally import client, which re-exports the useful shared types.
Keep the existing server package and binary name. Add a small `tanuki-js`
binding crate only when implementing JavaScript support; do not create an
empty package now. Server integration tests can use client as a dev dependency.

This split is justified by real reuse and browser dependency isolation. Making
client depend on the current server library would bring Axum, filesystem and
native Tokio features into its dependency graph. Separate domain, codecs,
transport traits and projections into modules rather than more packages.

Extract reusable primitives selectively: `Value`, finite floats, paths,
selectors/selections, client names, timestamps, nonnegative durations, expiry
and release policies. Keep `SessionHandle`, `WriteContext`, authoritative nodes,
claim creation and mutation policy on the server. Shared observed identifiers
do not grant authority. Avoid moving server-only constructors into a public
protocol API just to resolve cross-crate visibility.

`src/protocol.rs` currently imports core types for response conversion, and
request DTOs are private in `src/transport.rs`. Move wire definitions and
codecs into protocol; keep core-to-wire and wire-to-core conversion on the
server, using local conversion functions where orphan rules require it.
Move `SelectedView` to client. Update its tests/imports; do not add a server
runtime dependency on client just to preserve `tanuki::client`.

Preserve current wire bytes and error codes through extraction. Add typed
`WriteReceipt` decoding without changing its wire shape.
The raw WebSocket reply envelope can remain general internally; the client
decodes each result according to its pending request. Shared structural checks
must not become a second client-side implementation of server schema policy.

## Layer 1: direct protocol connection

`Connection` owns a WebSocket and codecs. It offers typed protocol send/receive
without allocating a retained cache, spawning a background driver, correlating
requests on the caller's behalf, or reconnecting. The caller explicitly sends
hello and observes every data message, including replies and errors.

```rust
let mut connection = Connection::open(ws_url).await?;
connection.send(ClientMessage::Hello {
    request_id: RequestId::new("hello-1".into()),
    client: name,
    selectors,
}).await?;

while let Some(message) = connection.recv().await? {
    match message {
        ServerMessage::Snapshot { .. } => { /* initialize */ }
        ServerMessage::Update { .. } => { /* inspect complete changes */ }
        ServerMessage::Reply { .. } => { /* correlate request */ }
        ServerMessage::Error { .. } => { /* inspect server error */ }
    }
}
```

Default outgoing codec is JSON; an explicit codec option also permits binary
MessagePack. Receive decodes by frame kind, preserving the current mixed-codec
contract. Close codes/reasons and transport failures remain observable.
WebSocket control-frame handling is transport plumbing, not application events.
The raw layer does not pretend it can resend hello to change selections.

## Layer 2: session and raw update listeners

`Session::connect` uses `Connection`, sends hello, installs the correlated
initial snapshot and starts one background driver. Startup warnings remain
available to the caller. It owns request IDs and correlation for writes, and
maintains the selected cache needed to attach listeners later. Passing an empty
selection is valid for a producer. An advanced caller can stay at `Connection`
if it wants full manual message/correlation control without this machinery.

```rust
let session = Session::connect(ws_url, name, selection).await?;
inspect_warnings(session.startup_warnings());

let mut updates = session.listen_updates().await?;
initialize(updates.initial_snapshot());
while let Some(batch) = updates.recv().await? {
    process_changes(&batch.changes); // Complete protocol update, no shaping.
}
```

Each `listen_updates()` call gets an independent receiver. It returns a current
local snapshot at registration plus complete subsequent server update batches;
that local baseline is not fabricated as a new server-correlated reply. A late
listener gets no earlier occurrences or commit history. Listener registration
and baseline capture are serialized with incoming update processing, so a
change falls in either the baseline or the following stream without a gap.

`Session::write(batch)` is the request-correlating primitive for atomic writes.
Replies and updates do not compete for one consumer channel. Raw listeners
receive only updates; the direct connection exposes the complete message stream.

Offer `publish` (retained output), `emit`, `submit_desired`, `submit_command`,
`define_input`, `claim_input`, `clear_desired`, and `remove` conveniences. All
are single-operation batches using the same path as
`write`; a multi-operation batch preserves operation order and rejects empty
construction. Server validation still applies to every received request.

Expiry is explicit in retained-write methods: `Preserve`, `Clear`, or
`Set(duration)`. `Preserve` keeps an existing absolute deadline; it does not
refresh a previous relative duration. Instant operations have no expiry field.
No implicit create/claim/bind behavior is added to typed payload helpers.

HTTP, SSE and schema/link management clients are outside this proposal's
current scope. No HTTP dependency or client pool is needed. Existing server
endpoints and their behavior are unchanged.

## Layer 3: observation helpers

```rust
let brightness = session.state::<u8>("/lamp/brightness")?;
let hue = session.state::<u16>("/lamp/hue")?;

let mut brightness_values = session.observe(&brightness).await?;
let mut lamp = session.observe_batch([hue.path(), brightness.path()]).await?;

while let Some(snapshot) = lamp.recv().await? {
    render(snapshot.read(&hue)?, snapshot.read(&brightness)?);
}
```

Both helpers use `listen_updates()` plus the same projection machinery. A
projection task starts from that listener's baseline, selects the requested
topics, applies each matching update completely, then sends an immutable full
snapshot through its output channel. The projection cache belongs to that task;
consumers never hold a lock into it. Each emitted snapshot retains its own
sequence and does not change when a later batch arrives. Start with owned
snapshots or `Arc<Snapshot>`; avoid a persistent data structure without evidence.

The first output is always the snapshot captured at registration, even if empty;
hold that baseline separately so a fast producer cannot overwrite it before
the first receive. Subsequent receives yield the latest unseen complete state.
Batch output contains all observed retained nodes, not only the ones changed
in that commit.
A two-topic commit yields one snapshot after both changes. Later outputs follow
matching commits, including metadata-only updates and duplicate payload writes.
An occurrence-only matching commit can yield an unchanged node map with a new
sequence; its occurrence payload is available only on the raw update stream.
Never store occurrences as current values or replay them in an initial snapshot.

The projection publishes after each matching batch, but its output channel
retains only the latest snapshot. A slow caller can skip intermediate revisions,
including a temporary removal followed by recreation; it still receives a
coherent state, never half a batch. Applications requiring every transition use
the raw listener. Numeric sequence gaps alone do not reveal coalescing because
server selection can already skip unrelated global commits.

`observe(&topic)` is the single-topic form: it emits an observed node containing
the sequence and an optional complete node. Absence/removal, a desired node
without a current payload, and a current null remain different states. Typed
handles provide fallible payload decoding in the first delivery; cross-topic
Serde shapes come later. `observe_batch()` initially takes
an explicit list of topic paths, deduplicated, avoiding a new selector-subset
algorithm. A future selection-based helper can share the same projection.
Wildcard-selected raw updates are already available through `listen_updates()`.

The session's hello selection bounds what can be observed. Validate each exact
observation topic with the existing selection matcher; return `NotSubscribed`
for an uncovered topic rather than reporting it as absent. A covered topic
that has not been created is a valid empty observation. Local observer creation
and dropping never change the server selection or open a second socket.
Cross-topic Serde shapes are a later adapter over complete snapshots; they do
not belong in the WebSocket driver or introduce new server semantics.

The session driver retains one selected cache for late registration; projection
tasks retain their own subsets. This intentionally duplicates some data to keep
helpers built on the same public primitive clients can use. Optimize only if
real observer counts or payload sizes justify sharing more state.

## Typed payloads and retained views

Consumer-facing handle sketch: `session.state::<T>(path)` returns a clonable
typed handle with `publish(value, expiry)`; event handles offer `emit`, desired
handles offer `submit` and explicit definition/claim methods, and command
handles offer `submit`. Handles refer to the existing session and do not own
additional sockets or keep a dropped session alive. Creating a handle performs
no server mutation or schema installation. Observation helpers accept handles;
batch snapshots can decode individual entries using those same handles.
These method/type names remain illustrative, pending implementation review.

Provide ordinary Serde-based `to_value<T: Serialize>` and
`from_value<T: DeserializeOwned>` helpers alongside explicit `Value` operations.
A Rust struct is one topic's map payload. It does not bind fields to child
topics, register a schema, or guarantee that other writers use the same type.

The conversion contract is independent of the wire codec:

- Scalars, string-keyed maps, lists, structs and Serde enum representations map
  to the corresponding Tanuki values. Unsigned Rust values are accepted only
  when representable by the signed 64-bit domain; overflow is an error.
- Reject non-finite floats and non-string map keys. Never round large integers
  or convert invalid floats to null.
- Preserve explicit Serde byte serialization as bytes; ordinary `Vec<u8>` may
  serialize as a list. Document `serde_bytes` for byte fields.
- Literal fields named `$bytes`, `$int`, etc. remain map fields. Only the wire
  codec interprets/escapes Tanuki tags; do not serialize a user struct to JSON
  and feed it into the tagged wire-value decoder.
- First delivery offers semantic timestamps/durations through explicit `Value`
  variants. Arbitrary Jiff fields use their own Serde representation; do not
  infer a semantic timestamp from a string. Semantic fields inside otherwise
  derived structs need a separately specified adapter and are deferred.
- Plain `Option<T>::None` encodes a payload null. Clearing desired state is an
  explicit operation, not a consequence of serializing `None`.

Evaluate `serde-value` as the small implementation bridge: it preserves numeric
variants and bytes and supplies serialization/deserialization machinery. Map
its value tree to Tanuki's narrower domain. Verify enum, option and numeric
conversion behavior before choosing a version. This avoids writing a general
Serde implementation unnecessarily. If its behavior does not fit, document the
concrete failing scenario before substituting a custom adapter.

The retained view stores raw, validated client nodes with private invariant
fields. Expose read-only iteration, lookup, sequence and metadata. Decode a
single node with an API equivalent to:

```rust
fn decode<T: DeserializeOwned>(
    &self, topic: &TopicPath,
) -> Result<Option<Node<T>>, PayloadDecodeError>;
```

`None` means no node. `Node::Desired` has `Option<Current<T>>`, and
`Current<Option<T>>` may itself contain a null payload. Event/command node
variants have metadata/definition but no retained payload. Preserve this
structure rather than collapsing all cases into a single optional value.
Decode errors include the topic and preserve the underlying cause; they do
not terminate subscription delivery or discard raw data. Warnings remain
successful-operation diagnostics.

## Driver, channels and lifecycle

One background driver owns the managed socket, request correlation, selected
cache and listener registration. It reads continuously regardless of application
polling. Processing a server update means checking/decoding the complete batch,
applying it atomically to the selected cache, then distributing the unchanged
batch to raw listeners. Observer registration is another serialized driver
command: capture the current cache and install a receiver before processing
another update. No cache read followed by a separately scheduled subscription.

Each projection is a task consuming a raw listener, with a local cache and an
output sender. Conceptually, `observe_batch` composes `listen_updates` with
`project_snapshots`, and the single-topic helper shares that implementation.
There is still only one task reading the WebSocket. Neither a full consumer
queue nor user callback can block that socket reader.

Replies resolve their own pending requests. An unrelated update may arrive
before a reply, and the server sends a write's reply before its corresponding
update to that same session. Therefore `write().await` promises acceptance,
not that observers have processed it. Filtered global sequence gaps are valid;
duplicates/regressions within a connection are protocol failures. Occurrences
are never added to either cache.
An uncorrelated server error must not disappear because no pending request owns
it: the helper layer reports it as a session failure. The direct protocol layer
continues to expose the original message for callers choosing their own policy.

Raw listeners use bounded queues and explicitly fail with `Lagged` on loss.
Terminate that listener after reporting the failure rather than continuing its
delta projection from an incomplete history. A fresh local registration can
recover current retained state but cannot recover missed occurrences. Only the
affected listener/projection ends; other listeners, requests and the session
remain active. A slow dashboard must not implicitly release controller claims.
If the socket itself is lost or the server disconnects a slow session, all its
listeners terminate with that reason.

Snapshot output uses the user-selected latest-only contract. Replacing an
unread full snapshot is normal and does not report lag or block projection.
The projection must still consume all input deltas; losing an input delta
requires termination and fresh registration, not simply skipping to the next
delta. Thus an application that polls snapshots slowly is supported, while a
projection task unable to process raw batches fast enough fails explicitly.

Candidate native machinery is Tokio bounded `mpsc` or `broadcast` for batches,
and `watch` for latest-only snapshots. Tokio
`broadcast` reports lag but permits subsequent receives: the SDK would make lag
terminal for its delta listeners. Keep concrete channel types private so
portable receiving/closure/error behavior can be implemented on WASM too.
Provisional limits are 64 pending requests, 64 queued raw batches per receiver,
one latest snapshot per observer (plus its undelivered initial baseline), and
the existing 1 MiB wire-message limit. Full local snapshots can be larger than
an update; these counts are not a total decoded-memory budget. Consumers may
also retain previously received snapshots. An implementation wrapping `watch`
must atomically read and mark a version seen, and explicitly deliver the first
baseline; the native channel's initial value is otherwise considered seen.

The session is the lifetime owner. Its driver runs in the background but is not
an unowned fire-and-forget task. Dropping an observer cancels its projection and
unregisters its listener, without closing the session. Dropping the session
cancels its driver and projections; observer/writer handles cannot keep it
alive. `close().await` closes and joins native tasks. Avoid library-owned runtimes
and blocking destructors. Terminal reasons have a separate path from full data
queues; receivers report an abnormal termination once and then end. Previously
delivered snapshots remain historical data, not evidence of a live connection.

Dropping a pending write future does not undo a submitted mutation. Remove
cancelled waiters and tolerate their late correlated replies without leaking
entries or delivering them to another request. Registration cancellation must
also clean up a receiver that its caller never acquired.

Use typed errors for endpoint/configuration, payload conversion, codec/protocol,
remote rejection, transport, timeout, queue exhaustion and session termination.
Preserve remote error codes/messages and transport sources. Distinguish a
known local pre-send failure from `OutcomeUnknown` after transmission may have
started. A lost reply or deadline after sending does not establish rollback.
Provisional connect/hello and per-request deadlines are 10 seconds, configurable;
there is no idle-update deadline.

Do not automatically retry mutations, reconnect, reclaim inputs or replay
commands in the first version. Reconnection is an explicit new session with a
fresh snapshot and sequence baseline; applications reclaim deliberately.
Surface close 4001 as replacement and 1013 as slow consumption. Automatic
reconnection after replacement would otherwise let duplicate names repeatedly
kick each other. Writes acknowledge core acceptance, not execution or durability.

The v1 server accepts selectors only in hello. Do not widen a selection by
opening another same-name socket: it would replace the current managed session.
A second independent session needs another caller-selected name. Dynamic
subscription changes would be a separate protocol proposal.

## Browser WASM path

Keep protocol, payload conversion and selected-view code independent of Tokio,
threads, filesystem and sockets. Use target-specific adapter modules inside
client: Tokio/Tungstenite for native WebSocket, browser WebSocket through
`gloo-net` or `web-sys` for WASM, and a target-appropriate task spawn/timer seam.
Prefer concrete target-selected implementations; no public transport trait,
`async-trait`, dynamic dispatch or unconditional `Send` future requirement is
needed. Native spawned tasks still satisfy their executor's bounds.

Choose and compile-check exact dependencies on the existing nightly pin during
implementation. Do not upgrade nightly or claim browser support from design
research alone. Add a protocol-only `wasm32-unknown-unknown` check early.

Later `tanuki-js` exposes Promise-returning operations and an async iterable of
whole batches plus TypeScript declarations. It delegates to client rather than
reimplementing requests or state application. Initial JavaScript target is a
browser; Node.js and WASI support require separate checks.

Proposed JavaScript mapping: ordinary values stay ordinary; signed 64-bit
integers use `bigint` when needed, bytes use `Uint8Array`, and timestamps and
durations use explicit wrappers retaining exact textual precision. Reject
unsafe integer `number` inputs instead of guessing their intended exact value.
Preserve `current: null` versus `current: { value: null, ... }`. Maps with
tag-looking keys remain literal data. Sequence/session identifiers should use
`bigint` consistently. Decode incoming wire JSON in Rust before converting to
JS, including metadata's u64 fields; JS `JSON.parse` would lose their precision.
Never round-trip those objects through `JSON.stringify`.

Browser deployment must be exercised against an actual server. Plan a same-origin
reverse-proxy example and an appropriate secure WebSocket endpoint for secure
pages. WASM does not remove browser transport restrictions. Name
collisions across tabs follow the ordinary managed-session replacement rule;
the application supplies a distinct name when independent tabs are intended.

## Proposed Iced consumer

Iced is the user's primary intended use case, not an SDK dependency or verified
deployment. Keep the core API useful to ordinary async Rust applications; a
dedicated Iced adapter or full GUI is not required. A native Iced subscription
can own the session and forward snapshot observations as application messages.
It sends a clonable
typed desired/command handle to application state; write gestures launch an
Iced task using that handle. Slider drafts, write acknowledgements and observed
actual state remain separate. The subscription worker retains the session
owner while those handles are in use.

Use stable subscription identity from connection configuration (and an explicit
attempt identifier if reconnect is added); UI value changes must not recreate
the connection. Returning no subscription cancels the worker and releases its
session. Forward errors and clean closure explicitly, and clear disconnected
write handles. No automatic reconnect/retry is implied. Bound the UI bridge
queue; it consumes latest snapshots while Tanuki continues updating its cache.
Decode failures can be sent as messages without ending raw observation.

Batch observation sends one complete application message for a coherent update
of multiple widgets. Cross-topic Serde shapes are still optional later work.
Native topic handles and receiving futures must work with Iced's executor
bounds, while browser counterparts must not gain unconditional `Send` bounds.
Offer ordinary async receiving and a standard `Stream` implementation/adapter
where practical; do not expose Iced messages or concrete channels as SDK types.

References checked for the sketch: [Iced 0.14 subscriptions](https://docs.rs/iced/0.14.0/iced/struct.Subscription.html)
and [tasks](https://docs.rs/iced/0.14.0/iced/struct.Task.html).

## Implementation handoff and acceptance

The current request is to prepare the handoff, not implement it in this turn.
When the user resumes implementation, use the task cards and decision status in
[client-api-handoff.md](client-api-handoff.md). Do not reopen agreed scope or ask
for repeated approval of routine choices. Public names and stated provisional
defaults can be refined during implementation without changing the contracts.

1. Extract protocol/primitives and migrate server conversions. Preserve existing
   JSON/MessagePack fixtures, parsing invariants and the full server suite.
   Check protocol's browser target without native server dependencies.
2. Add direct WebSocket connection send/receive, explicit hello and both codecs.
   Exercise the actual server's snapshot/update/reply/error messages, wire
   correlation and close reasons without any helper cache.
3. Build Session on that connection: background reader, request correlation,
   selected cache, and independent raw listeners with atomic baseline handoff.
   Test concurrent late registration and updates through deterministic ordering.
4. Build observation helpers on the raw listener primitive. Exercise US-09
   coherent snapshots and US-07 desired/atomic output flows through real sockets
   and production server logic. Add typed per-topic publication/conversion;
   reserve cross-topic Serde shapes for later. Cover map tags, overflow and bytes.
5. Test lifecycle failures: reply/update interleaving, lost replies, cancellation,
   observer lag isolation, full queues, replacement, clean shutdown, empty
   producer selection, reconnect after server sequence reset, and typed decode
   failure without invalidating the raw stream.
   Use deterministic transport seams only for failures difficult to inject;
   real server integration tests remain the authority for server behavior.
6. Deliver runnable publisher and controller/dashboard examples, workspace fmt,
   Clippy, unit/integration/end-to-end and release checks. Update architecture,
   protocol usage, procedures, toolchain and story test traceability as code lands.
7. Subsequent scope: browser adapter, JS binding, real headless-browser tests
   against production server, and TypeScript examples. A target compile check
   alone is not browser or JavaScript acceptance.

Typed round-trip cases must include null versus absent desired payload, literal
reserved-tag maps, i64 limits, bytes, enum shapes and decoding failures. Stream
cases must include removals, metadata-only changes, ordered occurrences, atomic
paired state, legitimate sequence gaps and explicit empty snapshots. Test late
local registration, latest-state coalescing without skipped input deltas,
independent overlapping observers, covered-but-absent versus
unsubscribed topics, local snapshot immutability, and observer drop separately
from managed duplicate replacement and whole-session closure. Existing stateless
HTTP tests remain server regressions; an HTTP SDK is not part of this delivery.

No Rust SDK, WASM support or deployment is claimed complete by this proposal.

## API documentation consulted

- [Tokio broadcast delivery and lag](https://docs.rs/tokio/latest/tokio/sync/broadcast/index.html)
- [Tokio latest-value watch semantics](https://docs.rs/tokio/latest/tokio/sync/watch/index.html)
- [Tokio bounded mpsc channels](https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html)
- [Gloo WebSocket futures API](https://docs.rs/gloo-net/latest/gloo_net/websocket/futures/struct.WebSocket.html)
- [wasm-bindgen WebSocket example](https://wasm-bindgen.github.io/wasm-bindgen/examples/websockets.html)
- [serde-wasm-bindgen conversion configuration](https://docs.rs/serde-wasm-bindgen/latest/serde_wasm_bindgen/struct.Serializer.html)
- [serde-value value algebra](https://docs.rs/serde-value/latest/serde_value/enum.Value.html)
- [Serde byte specialization](https://docs.rs/serde_bytes/latest/serde_bytes/)

These establish candidate capabilities, not compatibility with the repository's
pin. The implementation must verify actual versions and feature configurations.

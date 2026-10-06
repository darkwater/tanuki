# Client SDK implementation handoff

Prepared 2026-10-06 for a subsequent implementation session. Read this first,
then [client-api-design.md](client-api-design.md) for rationale and sketches.
This turn prepared documentation only. When the user asks to implement, start
at SDK-01 below and follow the requested scope; do not repeat design discovery.

## User decisions

- Build a reusable client crate. Focus only on WebSocket for now.
- Begin close to the actual protocol, with low-level control available.
- Build convenience helpers on those same primitives, not separate transports
  or duplicate protocol implementations.
- The user likes typed topic handles and writing through those handles.
- A background session task receives messages and distributes updates through
  channels. Offer raw updates, single-topic observation and batch observation.
- Observation helpers maintain task-local caches and deliver complete selected
  snapshots. Snapshot consumers get the latest complete state and may skip
  intermediate snapshots. Raw update listeners preserve batches and report lag.
- Typed payloads come first. Serde shapes spanning topic trees come later.
- Native Rust first; account for browser WASM now, implement its transport/JS
  bindings later. WASM is desirable, not a prerequisite for native delivery.
- Iced is the primary intended use case. Do not overfocus on it or make the SDK
  depend on it. Ordinary async Rust clients must remain straightforward.
- The design was prepared before switching to a cheaper implementation model.
  No SDK code, dependency changes or compiled API examples exist yet.

## Read and preserve

Follow [AGENTS.md](../AGENTS.md), including behavioral TDD, typed errors, nightly,
documentation maintenance and the existing server's validation boundary.
Read [spec.md](spec.md), [protocol.md](protocol.md),
[implementation-plan.md](implementation-plan.md), [design.md](design.md),
[test-plan.md](test-plan.md), and the user-story index and relevant US-01,
US-07, US-09 and US-12 files. Review accepted versus proposed policies in
[decisions-to-review.md](decisions-to-review.md). This SDK sequence is additional
work against the implemented server, not a request to repeat tasks 00–12.

Existing foundations:

| File | Useful starting point |
| --- | --- |
| `tanuki/src/protocol.rs` | Value codecs, snapshot/update/node/error DTOs; currently imports core types |
| `tanuki/src/transport.rs` | Private hello/write request DTOs and operation conversion; current wire behavior |
| `tanuki/src/client.rs` | `SelectedView` applies complete batches, separates occurrences and checks sequence order |
| `tanuki/src/domain/` | Paths, selection matching, values, identity and operation invariants |
| `tanuki/tests/protocol_json.rs`, `tanuki/tests/protocol_subscription.rs` | Existing codec and stream-shape contracts |
| `tanuki/tests/client_view.rs`, `tanuki/tests/websocket_api.rs` | Atomic local views and real-server socket scenarios |
| `Cargo.toml`, `tanuki/Cargo.toml`, `rust-toolchain.toml`, `docs/toolchain.md` | Workspace, exact dependencies and `nightly-2026-10-01` pin |

At handoff, the worktree contains design edits to this document,
`docs/client-api-design.md`, `docs/implementation-plan.md`, and stories 01, 07,
09 and 12. Preserve these edits. No commits were made for the design work. Check
the actual worktree before changing it; it may have changed since this handoff.
If Git's existing fsmonitor socket prints an error, use
`git -c core.fsmonitor=false ...` rather than editing repository configuration.

## Target shape, with names still provisional

Create `tanuki-protocol` and `tanuki-client` workspace members alongside the
existing `tanuki` server package. Server and client depend on protocol; client
must not depend on the server. Keep server authority/session handles and schema
policy server-side. Move shared wire DTOs and selected primitives, not the whole
server domain. No extra crate per small module. An Iced or JS package is not
needed now.

The intended consumer experience is approximately:

```rust
// Low-level caller controls hello, request IDs and every server message.
let mut connection = Connection::open(ws_url).await?;
connection.send(hello).await?;
let message = connection.recv().await?;

// Ordinary applications use a session and typed handles instead.
let session = Session::connect(ws_url, name, selection).await?;
let hue = session.state::<u16>("/lamp/hue")?;
let brightness = session.state::<u8>("/lamp/brightness")?;
let desired = session.desired::<u8>("/lamp/desired-brightness")?;

brightness.publish(&70, ExpiryUpdate::Clear).await?;
desired.submit(&70, ExpiryUpdate::Clear).await?;

let mut batch = session.batch();
batch.publish(&hue, &120, ExpiryUpdate::Preserve)?;
batch.publish(&brightness, &70, ExpiryUpdate::Preserve)?;
let receipt = batch.commit().await?;

let mut raw = session.listen_updates().await?;
initialize(raw.initial_snapshot());
let next_complete_batch = raw.recv().await?;

let mut one = session.observe(&brightness).await?;
let mut lamp = session.observe_batch([hue.path(), brightness.path()]).await?;
while let Some(snapshot) = lamp.recv().await? {
    render(snapshot.read(&hue)?, snapshot.read(&brightness)?);
}
session.close().await?;
```

This sketch is not compiled; exact builders, method names and accessors can be
adjusted coherently. Creating a typed handle is local. It does not publish,
define, claim, register a schema or open a socket. Handles are cheap to clone
and bind writes to their session. Do not silently accept another session's
bound handle into a batch; validate that boundary or make it unrepresentable.
Typed read failure is possible if another writer changes the kind/payload.
Expose metadata and raw values even when typed decoding fails.

The low-level `Connection` has no background cache/driver. `Session` builds on
it to manage hello, correlation, a selected cache and independent listeners.
Observation projections consume the public raw-listener primitive. The session
cache enables late registration; projection caches hold their selected state.
Some duplicated storage is an acceptable starting tradeoff.

## Contracts the implementation must preserve

1. Hello's correlated snapshot is the first successful session response. Expose
   its warnings. Startup selectors form one union and can be empty.
2. Local listener registration captures a current baseline and installs delivery
   at one serialized driver boundary. Updates cannot fall between those steps.
   Late listeners get current retained state, not historical occurrences.
3. Apply every complete update before projecting/publishing a snapshot. Raw
   consumers receive complete ordered batches. Events and commands never enter
   the retained cache. Preserve removal information and missing-versus-null.
4. Snapshot observation has a latest-state output; a slow renderer cannot block
   the socket reader or projection. The projection must consume every delta.
   Losing input deltas is different from replacing an unread output snapshot.
5. Numeric global sequence gaps are legal. Compare order within one connection,
   not across reconnects/server restarts. Neither gaps nor write receipt sequences
   establish that a selected observer missed a message or has caught up.
6. Replies are correlated independently of listeners. Write acceptance is not
   command execution, persistence, or confirmation that the UI observed it.
   Warnings are successful results, not errors.
7. Selectors cannot be changed after hello in the current protocol. Helpers
   observe within the existing selection. Distinguish an uncovered topic from
   a covered-but-absent one. Do not open a same-name socket to widen selection.
8. Value conversion preserves exact accepted integers, finite floats, bytes and
   literal tag-looking maps. Ordinary Serde data must not accidentally pass
   through the tagged wire decoder. Do not duplicate server schema validation.

## Provisional implementation defaults

These are working defaults from the proposal, not additional user decisions.
Use them unless an implementation issue warrants a small documented adjustment;
do not pause for approval of routine details.

- JSON outgoing by default, explicit MessagePack option, incoming codec from
  frame kind. Preserve current wire shapes and existing mixed-codec tests.
- Exact-path single/batch observers first, with duplicate paths removed and
  subscription membership checked using the existing matcher. Wildcard raw
  subscriptions remain available. No selector containment engine is needed.
- Deliver the registration snapshot once, even if empty, then latest unseen
  snapshots. Keep immutable historical snapshot values valid after later writes.
- Bounded raw queues; terminate only a lagging listener/projection with a typed
  reason. Fresh registration restores retained state but not missed occurrences.
  A whole socket failure terminates all its listeners.
- Session owns driver/projection lifetime. Dropping an observer cancels that
  projection; dropping/closing the session ends its tasks. Cloned topic handles
  do not keep a dropped session alive. Explicit close joins native tasks.
- Expose abnormal closure/lag even when a data queue is full. Report a terminal
  receive failure once, then end. Per-value typed decoding errors need not end
  an otherwise healthy observation.
- Start with 64 pending requests, 64 raw batches per listener, latest-only
  observer outputs, and the existing 1 MiB wire-message limit. These are not a
  total decoded-memory bound. Connect/hello and write deadlines: configurable
  10 seconds; no idle-observation timeout.
- No automatic retry, reconnect or reclaim. Distinguish known local pre-send
  failure from unknown outcome after transmission may have begun. Cancellation
  does not undo a write; clean up waiters and safely handle late replies.
- Use native Tokio channels/tasks internally, with concrete channel types kept
  private. Preserve a future browser adapter boundary without a public generic
  transport framework or unconditional `Send` bounds on browser code.
- Evaluate `serde-value`/`serde_bytes` rather than writing a general serializer
  without checking established APIs. Semantic time values remain explicitly
  available as `Value`; arbitrary derived time fields are not auto-detected.

## Bounded task cards

### SDK-01 — Extract the shared protocol boundary

Make the workspace and move shared DTOs/codecs/primitives. Keep core conversions
server-side and error sources intact. Avoid exposing authority constructors or
adding unchecked deserialization to get compilation green. Re-exports are fine
where they keep the dependency direction correct; do not give the server a
runtime client dependency. Keep `SelectedView` where convenient until SDK-03/05.

Use existing wire fixtures as regression evidence. Add serialization coverage
for request DTOs becoming bidirectional where missing. Run full server tests,
fmt and Clippy for this structural change. Inspect the protocol dependency graph
and attempt a protocol-only browser-target check. If target setup is unavailable,
report it accurately and continue native work; do not claim WASM support.

### SDK-02 — Direct WebSocket client

Implement `Connection` and explicit send/receive with JSON and MessagePack.
Use actual production server sockets to test hello/snapshot, writes/replies,
updates, errors, empty selection and server closure reasons. Do not add the
managed cache or helpers merely to test this layer. Reuse pinned compatible
Tungstenite where appropriate; verify all new dependency APIs/features.

### SDK-03 — Session driver and raw listeners

Implement hello lifecycle, request IDs/correlation, selected cache and independent
listener registration with baseline handoff. Test late registration concurrent
with commits, multiple consumers, unrelated updates interleaved with replies,
and writes completing without the application polling listeners. Cover queue
limits, cancellation, close/drop and replacement as these behaviors land.

### SDK-04 — Typed handles and atomic write helpers

Add state/event/desired/command handles, payload conversion, and a batch builder
with no effects until commit. All helpers use the same protocol write path.
Test Serde structs, enums, integer limits, invalid floats, bytes, reserved map
keys, and typed wrong-kind/invalid-value errors. Verify desired submission does
not claim, claims are explicit, warnings survive, and an invalid atomic batch
produces no partial state or occurrences.

### SDK-05 — Observation projections

Build single-topic and batch helpers on raw listeners. Test initial empty and
nonempty snapshots, no missed registration updates, metadata-only changes,
removal and desired payload clearing, immutable snapshots, and atomic paired
state. Demonstrate latest-output coalescing while all input deltas are applied.
Force projection input lag separately from a slow output consumer. Test that
one observer's failure/drop does not close another observer or its session.
Offer ordinary receiving plus a standard Stream adapter/implementation if it
fits without exposing runtime-specific channel types.

### SDK-06 — Consumer examples and native acceptance

Provide runnable ordinary Rust publisher and observer/controller examples. Use
the Iced interaction below as an ergonomics review; do not build a full GUI or
add Iced as an SDK dependency just to complete this task. Documentation can show
an Iced sketch and clearly distinguish it from a compiled/verified example.

Run the full workspace suite and release correctness checks. Use SDK clients in
real-server integration scenarios for US-01, US-07 and US-09. Server integration
tests can depend on client without making client depend on the server. Preserve
existing HTTP tests as server regressions. Update architecture, protocol usage,
procedures, toolchain, testing traceability and relevant stories.

For each meaningful behavior: write a behavioral failing test, implement, then
refactor. Do not claim a missing import as the red phase. Tests must exercise
actual server logic; narrow deterministic failure-injection seams can complement
real sockets for cancellation, lag and unknown-outcome tests. Synchronize on
explicit acknowledgements/registration/commits, not sleeps.

Baseline commands (add supported client/target configurations as they exist):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --release
```

## Primary consumer: Iced, without coupling

- An Iced Subscription owns Session and a latest-state observer. Its identity
  is stable connection configuration, not UI values. It forwards observations
  as application messages, one message per complete multi-topic snapshot.
- It sends a clonable desired/command handle to application state. UI gestures
  use Task::perform with that handle; no borrowing UI state across an await.
- Slider draft, write result/warnings and observed actual state are separate.
  Accepted submission does not mean a physical lamp changed.
- Bound the bridge queue. Slow UI forwarding must not block Tanuki projection.
  Surface decode errors, server closure and clean termination; clear dead handles.
- Subscription cancellation releases its session. If explicit reconnect is
  later shown, give attempts a distinct identity and ignore old-attempt messages.
- No SDK Iced dependency, Iced-specific event enum, integration framework or
  native-only public channel type. Check native Send/executor ergonomics without
  enforcing them universally on future browser futures.

Iced 0.14 documentation was consulted, but its example and the proposed SDK API
have not been compiled. The existing story has no deployed Iced application.

## Deferred and completion reporting

Defer HTTP/SSE SDKs, schema/link administration helpers, changing server selectors
mid-session, cross-topic Serde derives/shapes, Iced adapters, full UI work, browser
transport, JavaScript bindings, Node/WASI support, offline writes and automatic
reconnect/retry. Do not add replay, exactly-once delivery or durability promises.

Protocol/primitives should have a credible browser compilation path, but native
delivery must not expand into implementing browser support. Record any actual
portability limitation rather than solving hypothetical ones with abstractions.

At each task boundary report delivered behavior, test results, chosen defaults
and limitations. Do not call the complete SDK done after extraction or its happy
path. The design turn ran Markdown link/fence and Git whitespace checks only;
no Rust tests were run, and the current server baseline must be established by
the implementation session.

## Implementation follow-through — 2026-10-07

The earlier sections preserve the original design handoff. The native sequence
has now been implemented in order; see
[the compiled SDK guide](../tanuki-client/README.md) and
[acceptance traceability](testing.md#native-sdk-acceptance--2026-10-07).

| Task | Delivered and checked |
| --- | --- |
| SDK-01 | Three-package workspace; migrated validated primitives, bidirectional DTOs/codecs and shared complete-batch view; authority and core conversions stay server-side. Server regression suite and dependency graph checked. Browser-target attempt is blocked by uninstalled target std, without claiming WASM support |
| SDK-02 | Direct JSON/MessagePack Connection with explicit hello/send/receive, mixed-frame replies and close reasons; real production sockets |
| SDK-03 | Background reader plus bounded writer, hello warnings, typed receipts, cache/baseline registration, independent raw listeners, deadlines, cancellation, lag and joined native shutdown |
| SDK-04 | Four typed weak handles, explicit definitions/claims/submissions, staged atomic helpers, independent Serde conversion, per-value typed reads and raw metadata/Value access |
| SDK-05 | Single/exact-path batch projections built on raw listeners; initial baseline once, latest complete output, immutable history, metadata/removal/expiry handling, coverage errors and standard Stream adapters |
| SDK-06 | Ordinary Rust publisher/dashboard/controller examples; local compiled-example smoke against the production binary; real-server SDK acceptance for US-01/07/09 and full debug/release workspace gates |

Defaults retain JSON outgoing, explicit MessagePack, fixed hello selection,
64 pending writes, 64 raw batches, 1 MiB wire limit and 10-second deadlines.
Single observation uses the same `ObservationSnapshot` type as batches (with one
selected path). `read` of a path omitted from that projection reports
`NotObserved`. Clean closure drains admitted deltas and exposes final unseen
latest output; abnormal reasons preempt buffered data, report once and then end.
The writer has a one-second shutdown bound. Server-side message constructors
are conversion functions, since shared DTOs cannot depend on core types.

New fixtures found a pre-existing Serde buffering issue: whole MessagePack
requests, nodes and persisted nodes could lose codec context for bytes/extensions
or literal tag maps. Streaming private field DTOs now construct the public enums
without changing wire or persistence shape. The behavioral regression fixtures
and real socket checks include bytes, semantic times and literal `$bytes` maps.

Limits: the native transport currently enables plain `ws://`, without TLS;
serde-value's 128-bit integer methods are unsupported (narrow to i64 explicitly).
Arbitrary derived semantic-time adapters, total decoded-memory budgets and
cross-topic Serde shapes remain deferred. Browser std is not installed, so
browser portability is unverified. No browser bindings, Iced-specific code,
automatic reconnect/retry, replay, deployment or real hardware claim was added.

On 2026-10-07 the workspace was flattened to the user's usual convention:
`tanuki/`, `tanuki-protocol/`, and `tanuki-client/` directly at the root. The server
source/tests moved into its package; shared documentation and Cargo.lock remain
at the workspace root. References above now point to the current package paths.

# Architecture

This is the living implementation map. `design.md` contains the design
proposal and review sketches; this document records what the code actually
does.

## Current module map

| Module | Status | Responsibility |
| --- | --- | --- |
| `server` | tasks 04–08 implemented | Own process lifecycle, listener wiring, periodic/final saves, and shutdown |
| `client` | SDK-01 compatibility module | Re-export shared complete-batch `SelectedView` for existing consumers |
| `domain` | task 01 implemented | Paths, selectors, values, identities, nodes, and operations |
| `core` | tasks 02–11 implemented | Authoritative nodes, sessions, commits, claims, subscriptions, deadlines, schemas, linked views, and active diagnostics |
| `protocol` | SDK-01 server boundary | Re-export shared DTOs/codecs and convert authoritative core state |
| `transport` | tasks 04–07 plus SSE implemented | Axum HTTP/SSE/WebSocket lifecycle, codecs, routing, scheduler wakeups, and common errors |
| `scheduler` | task 07 implemented | Wait for the earliest value/claim deadline and invoke guarded core transitions |
| `persistence` | tasks 08–10 implemented | Versioned coherent node/schema/link snapshots, restore filtering, and atomic file replacement |
| `schema` | tasks 09 and 11 implemented | Typed ordinary-topic validators, explicit casts, overlap checks, freshness intervals, and named registry |
| `link` | task 10 implemented | Validated subtree definitions, topology checks, alias resolution, and enabled state |
| `diagnostics` | task 11 implemented in core | Derived read-only active conditions under `/$diagnostics/**`; transient warnings remain in outcomes/logging |

`server` provides the lifecycle seam. `domain` contains the accepted checkpoint
A representation and has no transport or storage dependencies. The remaining
modules are introduced by their implementation tasks rather than as empty
scaffolding.

## Dependency direction

The server dependency flow is transport → core → domain, with protocol DTOs
and selected primitives supplied by `tanuki-protocol`.
Domain code will not depend on Axum, sockets, or disk. Persistence and network
I/O will happen outside the authoritative state transition. The first server
seam accepts an injectable shutdown future so tests exercise the production
lifecycle without process signals.

## Checkpoint A — accepted 2026-10-05

The specification fixes four semantic node kinds, separates managed session
authority from client-name attribution, and separates input definition, claim,
and submission. The following sketch makes those facts explicit without using
independent booleans that can disagree:

```rust
enum Node {
    State(StateNode),
    Event(EventNode),
    Desired(DesiredNode),
    Command(CommandNode),
}

enum WriteContext {
    Stateless { client: ClientName },
    Managed(SessionHandle),
}

enum WriteOperation {
    PublishState { topic: TopicPath, value: Value, expiry: ExpiryUpdate },
    PublishEvent { topic: TopicPath, value: Value },
    DefineInput { topic: TopicPath, kind: InputKind, definition: InputDefinition },
    ClaimInput { topic: TopicPath, release: ClaimRelease },
    SubmitDesired { topic: TopicPath, value: Value, expiry: ExpiryUpdate },
    SubmitCommand { topic: TopicPath, value: Value },
    ClearDesired { topic: TopicPath },
    RemoveNode { topic: TopicPath },
}

struct WriteBatch(NonEmptyVec<WriteOperation>);
```

An unclaimed input can be created by placing `DefineInput` and its submission
in one atomic batch. Submission never claims it. `ClaimInput` accepts only a
live `Managed` context; matching stateless client names carry no claim
authority. Claims store an internal session identity and unique claim ID so stale
disconnect cleanup cannot release a replacement.

Accepted creation policy: each publication operation creates its corresponding
output kind when absent; `DefineInput` creates its requested input kind when
absent; submissions require an existing definition. On a freeform topic, an
operation selecting a different node kind replaces the old node and returns a
warning. This deliberately discards state that cannot exist in the new kind,
such as a retained payload, input definition, or claim. When an applicable
schema exists, its policy must permit the change; a denying result rolls back
the complete batch.

Accepted claim-collision policy: a live managed session may replace another
claim with a warning. This matches permissive output takeover while keeping the
claim/session distinction explicit. The displaced session immediately loses
claim authority.

Accepted initial value profile: null, boolean, signed 64-bit integer, finite
64-bit float, string, inline bytes, list, string-keyed map, timestamp, and
signed fixed duration. Unsigned integers and calendar-relative spans wait for a
demonstrated use. Timer parameters use a separate nonnegative fixed-duration
type. A missing desired payload remains distinct from `Value::Null`.

Accepted path profile: absolute UTF-8 paths; `/` is a virtual, non-writable
root; no empty, `.` or `..` segments. Ordinary topic segments reject the
reserved selector characters `*`, `?`, `[`, `]`, `{`, `}`, and `\`. A first
segment beginning with `$` belongs to Tanuki's reserved system namespace.
Selectors support whole-segment `*`, whole-segment recursive `**` (including
zero segments), non-nested whole-segment brace choices, and unions represented
by a `Selection` collection rather than parsing a `+` operator.

The corresponding C1/C2 contract is: a same-name stateless write cannot gain a
managed session's authority; replacing a managed session invalidates its old
handle; output takeover succeeds with a warning; input submission preserves
the claim; and any invalid operation rolls back the complete mixed state/event
batch before an occurrence becomes observable.

The accepted kind-change tradeoff favors convenient freeform replacement. The
warning makes loss of retained payloads, definitions, or claims visible without
turning an ad hoc topic into a permanently locked type. Denying schemas retain
the ability to enforce a stable shape where it matters.

## Core mutation and sessions implemented in tasks 02–03

`Core::apply` is the only state mutation boundary. It validates system-topic
rules, clones the current in-memory map into a candidate,
applies every operation against that candidate, and installs it only when all
operations succeed. An error discards candidate nodes, event occurrences,
warnings, and sequence advancement together. Successful output writes return a
single sequence-numbered `UpdateBatch`; network delivery is not part of this
transition.

Output ownership is derived from the latest accepted provenance rather than a
second owner field. Identical state writes still replace the retained wrapper,
refreshing timestamp and expiry. Event nodes persist publisher metadata but
never their occurrence payload.

Input definition, claiming, desired submission/clearing, and command submission
use the same candidate and commit path. Definition, claim, and submission are
distinct operations and can be composed in that order for one topic in a
single batch. Repeated operations in any lifecycle slot and output/removal
combinations execute in request order. The core coalesces their retained
effects against the pre-batch state while retaining all instant occurrences.

The core registry maps each managed client name to an opaque session ID. A
replacement invalidates the old handle before it can write again. Stateless
contexts carry attribution only and never enter or modify this registry.
Claims contain both the owner label and session ID: submissions may come from
any actor without transferring the claim, while definition changes to a
claimed input require the claiming session.

Disconnect and same-name replacement share one claim-release procedure.
Immediate release removes only the claim, preserving the input definition and
desired value in an optional atomic update. Grace release leaves the claim in
place and returns `(topic, claim ID, deadline)` work for the deadline scheduler;
the claim ID is the guard against clearing a later replacement. If a grace
deadline cannot be represented, the core warns and releases immediately.

Instant-output metadata remains a visibly provisional persistence default.
Same-topic batch behavior is accepted: operations execute sequentially, the
final retained shape is observed atomically, and occurrences are not dropped.

## Checkpoint B — stream contract accepted, remaining details tracked

`Core::subscribe` registers a selector union and captures its snapshot within
one exclusive core transition. A concurrent write therefore appears either in
that snapshot or in a newer queued update. Each subscriber receives a filtered
projection of the complete commit; overlapping selectors cannot duplicate a
change. Empty projections are not queued.

Core queues have configurable nonzero batch capacity and use `try_send`, so a
slow subscriber never blocks mutation or another subscriber. Overflow removes
only that subscription and records `SlowConsumer` through a separate closure
signal. Byte accounting and the proposed 1 MiB connection budget remain a
transport-boundary question in `open-questions.md`.

The wire model uses full `NodeView` values in upserts, explicit
event/command occurrences, and removals carrying the previous node. Both state
and desired nodes use a `current` wrapper. A successful correlated snapshot is
the subscription acknowledgement; subsequent updates carry the global commit
sequence. Filtered streams may skip irrelevant global sequence numbers.

`SelectedView` is a minimal client-side projection. It rejects duplicate or
older sequences before mutation, then applies all node upserts and removals
under exclusive access in one call and returns instant occurrences separately.
There are no fallible operations after the sequence check, so the client avoids
cloning the complete retained cache on every delta. Callers can inspect only
the completed batch. Exact queued-byte accounting remains open separately.

## WebSocket transport implemented in task 06

The Axum router owns a small connection registry separate from core state. A
hello frame chooses JSON text or MessagePack binary for unsolicited messages,
opens the managed core session, and atomically registers its selection. The first
server frame is the correlated snapshot. The registry exists only to signal a
displaced socket; session authority and replacement correctness remain owned
by `Core` and its opaque session IDs.

The socket loop selects among replacement, core updates, and client frames.
Writes use the same `Core::apply` path as HTTP. A reply is encoded immediately
after that call and before the loop can forward the writer's resulting update.
Core batches remain indivisible across both codecs. Disconnect invokes core
cleanup before conditionally removing only the matching registry entry, so an
old socket cannot remove its replacement.

## SSE transport

The read-only `/v1/sse` adapter registers its selector and captures the first
snapshot through the same atomic `Core::subscribe` call as WebSocket. It maps
the snapshot and every subsequent `UpdateBatch` directly to one JSON SSE
event, preserving the core's batch boundary. SSE is anonymous and allocates no
managed session or connection-registry entry.

Sequence numbers populate both the JSON view and SSE event ID for inspection,
but the adapter deliberately ignores `Last-Event-ID`. Dropping a response
drops its receiver. Core queue overflow closes the stream after queued batches
are drained and logs the slow-consumer reason; reconnect creates a fresh core
subscription rather than a replay session.

JSON retains the explicit semantic tag mapping. MessagePack carries bytes
natively, uses its standard extension type `-1` for timestamps, and uses Tanuki
application extension type `2` for durations. Each incoming frame selects its
own decoder and its correlated reply codec; the hello codec remains the codec
for unsolicited snapshots and updates. Both codecs deserialize to the same
transport-independent `Value` enum.
The connection has a 1 MiB per-message limit and its core subscription has a
64-batch nonblocking queue. A total queued-byte budget remains open.

## Deadline execution implemented in task 07

Retained values store absolute wall-clock deadlines. `Core::process_deadlines`
is the authoritative timer mutation boundary: expired output state becomes a
removal, while an expired desired payload becomes a full-node upsert with its
definition and claim intact. All changes due at one processing instant form
one commit and are published through ordinary subscriptions.

The Tokio deadline scheduler owns no domain state. It scans the core for the
earliest retained-value deadline and holds explicit pending claim-release work
returned by session disconnect/replacement. Every accepted retained write
wakes it, so a moved-earlier deadline replaces the current wait. On wake it
passes due claim IDs and current wall time into the core. Current deadlines and
claim IDs are rechecked there, making refreshed values and reclaimed inputs
immune to stale work.

The transport starts one scheduler alongside its shared core. Its clock is the
same injected wall-clock seam used for writes; Tokio's monotonic timer is only
used for waiting. Omitted wire expiry provisionally maps to `Preserve`: it
keeps an existing absolute deadline and yields no deadline on a new value.

## Best-effort persistence implemented in task 08

`SnapshotStore` captures one coherent canonical clone while holding the core
lock, then performs MessagePack encoding, file writes, fsync, and rename outside that
lock on a blocking worker. The version-3 file records its format marker,
sequence, save time, ordinary node data, absolute expiries, provenance, and
validated installed schemas and link definitions. Alias projections are derived
state and are never duplicated in the snapshot.
It never stores live sessions or input claims. Event publisher metadata and
command definitions persist, but occurrence payloads never do.

Restore validates the marker/version and invariant-bearing paths/names through
their deserializers. Expired state is omitted; an expired desired payload
restores as its definition without a current value. Provenance retains client
and timestamp but loses session identity. The persisted commit sequence is
continued so a post-restart mutation remains newer than restored snapshots.

The production server loads before serving, attempts a snapshot every 30
seconds, logs periodic failures while keeping the live core available, and
attempts a final save after graceful network shutdown. Malformed, invariant-
invalid, or unsupported configured snapshots are copied to a non-overwriting
adjacent `.bak`, logged, and treated as empty state; read/backup I/O errors
still fail startup. The default path is `tanuki.db`, overridden by
`TANUKI_SNAPSHOT`.

## Schema policy implemented in task 09

`ValueValidator` represents null policy, primitive kind checks, inclusive
integer/finite-float ranges, and string enums without encoding schemas into the
runtime `Value` representation. `SchemaRule` restricts broad selectors to
ordinary topics and rejects selectors that explicitly name a `$` branch.
Warning enforcement returns a successful diagnostic outcome; deny enforcement
returns a typed `SchemaViolation`.

`Selector::intersects` lets the registry reject overlap between separately
installed schemas. Rules within one schema may overlap and all apply, except
that intersecting casts are rejected. `Core::apply` is the single validation
site for every value-bearing adapter; a successful explicit cast supplies the
stored value and every matching validator rechecks it.

`Core::install_schema` stages replacement atomically. Normal installation
rejects existing deny violations without casting stored values; force mode
removes invalid state and clears invalid desired current values while retaining
input metadata. `PUT /v1/schemas/{name}` exposes complete declarations. The
registry is stored in the snapshot and reconstructed through invariant-checking
constructors on restore.

## Writable linked views implemented in task 10

`LinkDefinition` names one ordinary mount subtree and one ordinary canonical
target subtree. The initial topology rejects overlap between a link's source
and mount, overlapping mounts, targets reached through another mount, retained
destination collisions, and either endpoint in the reserved system namespace.
The registry is deliberately small and direct; link chains are deferred.

Snapshots and subscriptions project enabled aliases from canonical nodes rather
than storing copies. Canonical mutation, expiry, session cleanup, schema
changes, link replacement, and link removal derive all alias upserts/removals in
the same commit. Replacing or deleting a definition retracts its old projection.
Restore validates definitions, rebuilds the registry, and computes enabled
state from current canonical data.

An alias write applies alias policy and any explicit cast before translating
the operation to its canonical topic, where canonical policy and a possible
second cast apply. Disabled aliases still resolve repair attempts. Direct
canonical writes are never rejected solely for a linked-view violation: a deny
disables the whole view, emits last-visible removals and a diagnostic, and keeps
the canonical commit. A later valid relevant mutation or applicable schema
replacement re-enables the view atomically. Invalid instant occurrences remain
canonical-only; a later valid occurrence can recover the view, with no replay.

## Freshness and shared diagnostics implemented in task 11

An optional positive `expected_update_interval` on a schema rule applies to
retained state and present desired values. The strictest matching interval wins.
The deadline scheduler considers both value expiry and freshness deadlines, but
freshness never deletes or mutates the source node. Definition-only desired
inputs and event/command nodes have no freshness deadline.

The core derives active state nodes under `/$diagnostics/freshness/<source...>`
and `/$diagnostics/links/<link>`. These nodes use ordinary snapshot/update DTOs
but are Tanuki-owned, read-only, exempt from user schemas, links, expiry, and
persistence. A condition onset is an upsert and recovery is a removal in the
same commit as a caller-driven repair. Duplicate accepted writes refresh their
provenance even when the value is equal, removing an overdue condition and
scheduling the next deadline.

Diagnostic publication uses the existing nonblocking subscription path and
does not itself generate a diagnostic if a consumer overflows. This prevents a
diagnostic-consumer failure from recursively creating more diagnostics. Local
structured logging and per-operation warning arrays remain the channel for
transient conditions that are not retained active state.

## Native SDK boundary — SDK-01 through SDK-06

The workspace has three packages: the server package, `tanuki-protocol`, and
`tanuki-client`. Protocol owns validated paths/selectors/client names, values,
time/timer primitives, bidirectional wire DTOs, value codecs and `SelectedView`.
Server modules re-export migrated types. Authority-bearing session/claim handles,
node storage, schema policy and core-to-wire conversions stay server-side.
Server tests depend on client as a dev dependency; there is no runtime server →
client dependency and client never depends on server.

Payload-bearing tagged DTOs decode through private streaming field DTOs and
validated enum construction. Serde's internally tagged derive buffers values and
loses `is_human_readable`, breaking MessagePack bytes/extensions and literal tag
maps. Serialization and the v1 wire shape stay unchanged. Snapshot persistence's
payload-bearing enum uses the same approach, without changing its file format.

Client modules are concrete boundaries: `connection` owns native wire I/O;
`session` owns hello, correlation, selected state, registration and queues;
`payload` bridges ordinary Serde data without interpreting tags; `topic` provides
weak typed handles and staged writes; `observe` projects raw input into immutable
complete snapshots. `SelectedView` is shared with the server's original client
fixtures through re-exports, avoiding a second batch-application implementation.

One driver serializes incoming batches, pending replies and local registration.
Registration captures the cache and installs delivery before another driver step.
A bounded writer task keeps network backpressure outside the reader. Each raw
listener has a bounded data queue and independent terminal notification. Slow
raw input ends only that listener/projection. Replies resolve independently of
application polling. Cancellation releases pending capacity; late IDs are never
reused for another request.

Observers consume that public raw primitive, maintain task-local selected caches,
and publish latest complete snapshots. Initial baselines remain separate from
replaceable output. Some duplicated retained data is the deliberate simplicity
tradeoff; queue counts do not bound total decoded memory. Explicit exact-path
coverage lets typed reads distinguish unobserved paths from covered absence.

Session owns driver/writer and projection lifetime. Topic handles are weak;
observer drop aborts only its projection, session drop signals driver shutdown,
and explicit close joins tasks. Clean termination drains admitted raw batches and
exposes final unseen latest output; abnormal termination takes precedence over
queued data and is reported once. No replay, retry, reconnect, schema validation
or framework-specific integration lives in the SDK.

## Flat package layout — 2026-10-07

All three packages live directly below the repository root: `tanuki/`,
`tanuki-protocol/`, and `tanuki-client/`. The root manifest is a virtual workspace
and defaults test/check commands to all members. The server owns its `src/` and
`tests/` directories inside `tanuki/`; shared docs, toolchain pin, lockfile and CI
stay at the root. Use `cargo run -p tanuki` to select the server explicitly.
This matches the user's usual package layout and avoids treating the server as
structurally special. Runtime dependency direction is unchanged.

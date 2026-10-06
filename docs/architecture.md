# Architecture

This is the living implementation map. `design.md` contains the design
proposal and review sketches; this document records what the code actually
does.

## Current module map

| Module | Status | Responsibility |
| --- | --- | --- |
| `server` | tasks 04–08 implemented | Own process lifecycle, listener wiring, periodic/final saves, and shutdown |
| `client` | task 05 implemented | Maintain a selected local node view by applying complete update batches |
| `domain` | task 01 implemented | Paths, selectors, values, identities, nodes, and operations |
| `core` | tasks 02–07 implemented | Authoritative nodes, sessions, commits, claims, subscriptions, and deadlines |
| `protocol` | tasks 04–06 implemented | JSON/MessagePack values and typed snapshot/update/error DTOs |
| `transport` | tasks 04–07 implemented | Axum HTTP/WebSocket lifecycle, codecs, routing, scheduler wakeups, and common errors |
| `scheduler` | task 07 implemented | Wait for the earliest value/claim deadline and invoke guarded core transitions |
| `persistence` | task 08 implemented | Versioned coherent snapshots, restore filtering, and atomic file replacement |
| `schema` | task 09 foundation | Typed ordinary-topic validators and structured warning/deny results; installation pending |
| `links` | planned | Linked path resolution, visibility, and recovery |
| `diagnostics` | planned | Structured warnings/errors and logging integration |

`server` provides the lifecycle seam. `domain` contains the accepted checkpoint
A representation and has no transport or storage dependencies. The remaining
modules are introduced by their implementation tasks rather than as empty
scaffolding.

## Dependency direction

The intended inward dependency flow is transport/protocol → core → domain.
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

`Core::apply` is the only state mutation boundary. It validates system and
duplicate-target rules, clones the current in-memory map into a candidate,
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
single batch. Other repeated operations in the same lifecycle slot, and any
same-topic output/removal combination, are rejected as ambiguous.

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

Two implementation defaults remain visibly provisional pending later protocol
review: same-topic input lifecycle operations may compose once per lifecycle
slot while ambiguous repeats are rejected, and instant-output metadata remains
present in snapshots. These choices support atomic input creation without
introducing implicit last-write-wins or event replay.

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

`SelectedView` is a minimal client-side projection. It stages all node upserts
and removals in one call before replacing the visible map, returns instant
occurrences separately, and rejects duplicate or older sequences without
mutation. Same-topic batch repetition and exact queued-byte accounting remain
open separately; they do not change the accepted snapshot-then-update stream.

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

`SnapshotStore` captures one coherent selected clone while holding the core
lock, then performs MessagePack encoding, file writes, fsync, and rename outside that
lock on a blocking worker. The version-1 file records its format marker,
sequence, save time, ordinary node data, absolute expiries, and provenance.
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

## Schema validation foundation

The first task-09 slice is deliberately independent of schema installation.
`ValueValidator` represents null policy, primitive kind checks, inclusive
integer/finite-float ranges, and string enums without encoding schemas into the
runtime `Value` representation. `SchemaRule` restricts broad selectors to
ordinary topics and rejects selectors that explicitly name a `$` branch.
Warning enforcement returns a successful diagnostic outcome; deny enforcement
returns a typed `SchemaViolation`.

`Selector::intersects` computes whether any valid non-root topic could match
two patterns and is symmetric by construction. The future schema registry will
use it to reject overlap between separately installed schemas. Registry
installation, existing-value cleanup, within-schema overlap, casting, transport
DTOs, persistence, and core mutation-path enforcement remain pending the
questions in `open-questions.md`.

# Architecture

This is the living implementation map. `design.md` contains the design
proposal and review sketches; this document records what the code actually
does.

## Current module map

| Module | Status | Responsibility |
| --- | --- | --- |
| `server` | bootstrap | Own the process lifecycle and, later, runtime wiring and shutdown |
| `domain` | task 01 implemented | Paths, selectors, values, identities, nodes, and operations |
| `core` | tasks 02–03 implemented | Authoritative nodes, managed sessions, atomic commits, and input claims |
| `protocol` | planned | Versioned transport DTOs and codec conversion |
| `transport` | planned | Axum HTTP/WebSocket extraction and response mapping |
| `persistence` | planned | Versioned coherent snapshots and file operations |
| `schema` | planned | Ordinary-topic validation and freshness policy |
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
place and returns `(topic, claim ID, deadline)` work for the task 07 scheduler;
the claim ID is the guard against clearing a later replacement. If a grace
deadline cannot be represented, the core warns and releases immediately.

Two implementation defaults remain visibly provisional pending later protocol
review: same-topic input lifecycle operations may compose once per lifecycle
slot while ambiguous repeats are rejected, and instant-output metadata remains
present in snapshots. These choices support atomic input creation without
introducing implicit last-write-wins or event replay.

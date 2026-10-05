# Architecture

This is the living implementation map. `design.md` contains the design
proposal and review sketches; this document records what the code actually
does.

## Current module map

| Module | Status | Responsibility |
| --- | --- | --- |
| `server` | bootstrap | Own the process lifecycle and, later, runtime wiring and shutdown |
| `domain` | checkpoint A | Paths, selectors, values, identities, nodes, and operations |
| `core` | planned | Authoritative state, sessions, atomic commits, and subscriptions |
| `protocol` | planned | Versioned transport DTOs and codec conversion |
| `transport` | planned | Axum HTTP/WebSocket extraction and response mapping |
| `persistence` | planned | Versioned coherent snapshots and file operations |
| `schema` | planned | Ordinary-topic validation and freshness policy |
| `links` | planned | Linked path resolution, visibility, and recovery |
| `diagnostics` | planned | Structured warnings/errors and logging integration |

Only `server` exists in code. Domain and core modules wait for architecture
checkpoint A so their types do not accidentally freeze open product decisions.

## Dependency direction

The intended inward dependency flow is transport/protocol → core → domain.
Domain code will not depend on Axum, sockets, or disk. Persistence and network
I/O will happen outside the authoritative state transition. The first server
seam accepts an injectable shutdown future so tests exercise the production
lifecycle without process signals.

## Checkpoint A proposal — pending review

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
authority. Claims store an internal session identity and generation so stale
disconnect cleanup cannot release a replacement.

Proposed creation policy: each publication operation creates its corresponding
output kind when absent; `DefineInput` creates its requested input kind when
absent; submissions require an existing definition. Operations against a
different existing kind fail atomically. Kind replacement initially requires
an explicit removal followed by a later creation.

Proposed claim-collision policy: a live managed session may replace another
claim with a warning. This matches permissive output takeover while keeping the
claim/session distinction explicit. The displaced session immediately loses
claim authority.

Proposed initial value profile: null, boolean, signed 64-bit integer, finite
64-bit float, string, inline bytes, list, string-keyed map, timestamp, and
signed fixed duration. Unsigned integers and calendar-relative spans wait for a
demonstrated use. Timer parameters use a separate nonnegative fixed-duration
type. A missing desired payload remains distinct from `Value::Null`.

Proposed path profile: absolute UTF-8 paths; `/` is a virtual, non-writable
root; no empty, `.` or `..` segments; topic segments reject selector syntax.
Selectors support whole-segment `*`, whole-segment recursive `**` (including
zero segments), non-nested brace choices, and unions represented by a
`Selection` collection rather than parsing a `+` operator.

The corresponding C1/C2 contract is: a same-name stateless write cannot gain a
managed session's authority; replacing a managed session invalidates its old
handle; output takeover succeeds with a warning; input submission preserves
the claim; and any invalid operation rolls back the complete mixed state/event
batch before an occurrence becomes observable.

The main tradeoff is strict kind stability versus convenient replacement.
Rejecting implicit changes prevents accidental loss of retained payloads,
definitions, and claims, at the cost of requiring a deliberate two-step remove
and recreate operation when a topic's role genuinely changes.

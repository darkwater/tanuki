# Tanuki — proposed architecture and local API review

These are review sketches, not compiled code or frozen API requirements.
Undefined names are placeholders. Rust/nightly, TDD, typed errors and internal
invariants are user requirements; specific module and method names below are
recommendations.

## Module responsibilities

Start with one library and a thin binary. Split crates only when reuse or
dependencies justify it.

| Module      | Responsibility                                                               |
| ---         | ---                                                                          |
| domain      | Paths, selectors, values, identities, node structure and operations          |
| core        | Authoritative state, sessions, atomic mutation and subscription registration |
| schema      | Ordinary-topic validation, casts, rule installation and freshness policy     |
| links       | Path resolution, reverse indexes, visibility and recovery                    |
| protocol    | Request/reply/update DTOs and JSON/MessagePack mappings                      |
| transport   | Axum HTTP, WebSocket, extractors and response/error mapping                  |
| persistence | Coherent snapshot encoding and file operations                               |
| server      | Runtime wiring, expiry scheduling, shutdown and configuration                |
| diagnostics | Structured warning/error concepts and logging integration                    |

Domain must not depend on Axum. Network/disk operations must not occur while
authoritative state is locked or borrowed for a commit. Initial state can be
owned by a single core task receiving commands; deterministic core transition
methods remain callable directly in tests.

## A — Node structure

Use one enum as the authoritative source of node shape. Avoid kind flags that
disagree with independently optional fields.

```rust
enum Node {
    State(StateNode),
    Event(EventNode),
    Desired(DesiredNode),
    Command(CommandNode),
}

struct RetainedValue {
    value: Value,
    last_write: WriteProvenance,
    expires_at: Option<Timestamp>,
}

struct DesiredNode {
    definition: InputDefinition,
    claim: Option<InputClaim>,
    current: Option<RetainedValue>,
}

struct CommandNode {
    definition: InputDefinition,
    claim: Option<InputClaim>,
    // No retained payload or value deadline exists here.
}
```

StateNode contains a required RetainedValue plus output metadata. EventNode
contains output metadata but no last event payload. An instant occurrence lives
in the committed update batch. DesiredNode with no current payload differs from
one containing Value::Null. Metadata layout should avoid storing an output
owner separately if it is derivable from last-writer provenance.

InputClaim needs managed session identity plus a release policy; its owner name
can be resolved or captured without making name equality establish session
authority. Review how a claim records disconnected grace without dangling
access to a deleted registry entry.

## B — Identity and operations

```rust
enum WriteContext {
    Stateless { client: ClientName },
    Managed(SessionHandle),
}

impl Core {
    fn read(&self, selection: &Selection) -> Snapshot;
    fn open_session(&mut self, name: ClientName) -> OpenSessionOutcome;
    fn apply(
        &mut self,
        actor: &WriteContext,
        batch: WriteBatch,
        now: Timestamp,
    ) -> Result<CommitOutcome, CoreError>;
}
```

SessionHandle is issued by the core, not deserialized from an untrusted name.
Its constructor is private and its liveness is rechecked at mutation time.
Session replacement invalidates the old handle; matching names on stateless
writes never reuse its authority. The async CoreHandle wraps these transitions
with a command channel if that architecture is chosen.

WriteBatch is nonempty and contains explicit publication, input
definition/claim, or input submission intents. Separate definition and claim in
the operation representation if a stateless client can create an unclaimed
input. Do not force stateless HTTP to create a session just to fit a struct.
Different concrete operations may carry different applicable options; do not
put a payload expiry field on every command.

The exact operation enum is checkpoint A work: it must express creation of an
unclaimed input with its kind, submission to an existing input without
repeating its kind, and session-bound claiming. Unsupported kind changes need a
stated policy, not an accidental overwrite.

## C — Atomic mutation and validation

Proposed procedure:

1. Parse transport DTOs and convert to domain types. Construct attributed context.
2. In one serialized core operation, resolve target paths and applicable system rules.
3. Stage changes against a candidate view; check session/claim rules and direct schemas. Apply permitted direct casts, then revalidate.
4. Compute linked views against that candidate. Disable/re-enable links as needed without casting source values through linked schemas.
5. If a denying direct check fails, return a typed failure with no committed node or event effects.
6. Otherwise install the complete change and derive one batch of observable changes. Queue it to affected subscriptions before a later commit can overtake it.
7. Return success with warnings. Network delivery and best-effort disk saving proceed outside the state transition.

Validation failure may itself be logged even though data changes roll back. A
success acknowledgement is not command execution, durable persistence, or proof
that all subscribers received the batch.

A private prepared-change type may make staging/commit separation clear, but is
not a user-schema proof type. Preparation and commit must remain adjacent
within the same serialized operation. Reusing a prepared result after another
schema/state change is invalid unless revalidated. Do not introduce optimistic
transactions just for this split.

A debug assertion at commit may rerun a pure check against the same
candidate/policy when straightforward. Never make it perform the primary
validation.

## D — Observation API

```rust
enum Change {
    Upsert { topic: TopicPath, node: NodeView },
    Occurrence { topic: TopicPath, event: EventOccurrence },
    Removed { topic: TopicPath, previous: NodeView },
}

enum SubscriptionMessage {
    Snapshot(Snapshot),
    Update(UpdateBatch),
}
```

NodeView is a transport-neutral enum reflecting the four node kinds, without
exposing mutable internal state. Upsert can represent input value clearing or
claim-only changes. Removed always means disappearance of a previously visible
node, with its previous visible state. No removed boolean next to a misleading
current value. Events remain occurrences, never retained snapshots.

Subscribe must atomically register a selection and capture its snapshot. A
receiver can then see Snapshot followed by newer batches without a gap.
Consumers apply an entire batch before recomputing a shape. Internal commit
sequence numbers can help ordering/tests without promising durable event
replay.

## E — Errors, HTTP extraction and responses

```rust
enum CoreError {
    SessionExpired(SessionId),
    System(SystemViolation),
    Schema(SchemaViolation),
    InvalidOperation(OperationError),
}

struct Accepted<T> {
    result: T,
    warnings: Vec<Diagnostic>,
}

struct ApiResponse<T> { /* common success envelope */ }
struct ApiError { /* typed mapping to common error envelope */ }
```

These sketches omit thiserror derives and wire fields until reviewed. Keep
parse/codec failures, core failures and persistence failures in their own
meaningful error domains. A central API conversion maps them to stable wire
error codes, appropriate HTTP status and correlation information. WebSocket
uses the same semantic error codes without pretending every message is an HTTP
response.

Suggested extractors: validated client attribution, selection, and decoded
write request. Middleware handles request tracing and general limits. Both
extractor failures and handler failures use the common error body; early
middleware failures should too where Tanuki controls the response. Handler
responsibilities are extract, call core, return typed response. No duplicated
validation or storage writes in handlers.

## Review points and living procedures

Review A: node/context/operation types plus core mutation API, before implementing that model.

Review B: snapshot/batch/error wire shapes and subscribe boundary, before committing to WebSocket.

Review C: schema/link write-validation and automatic recovery procedures, before those features.

At each review use concrete examples: battery output, desired lamp input,
instant command, and a rejected atomic batch. Show roughly 30–60 lines of
relevant snippets, not the entire module tree. Continue unrelated
already-authorized work while waiting for architectural feedback.

The implementation must keep startup/restore, mutation, subscribe,
disconnect/replacement, expiry, link recovery and shutdown procedures
documented alongside code. This handoff starts those documents; it does not
replace maintaining them.

## Documentation references

Check these against the pinned toolchain/dependency versions during
implementation:

- [Nightly standard library](https://doc.rust-lang.org/nightly/std/)
- [Rust Unstable Book](https://doc.rust-lang.org/nightly/unstable-book/)
- [Return type notation design](https://rust-lang.github.io/rfcs/3654-return-type-notation.html) — background, not proof of current compiler support.
- [Axum response conversion](https://docs.rs/axum/latest/axum/response/trait.IntoResponse.html)
- [Axum request extraction](https://docs.rs/axum/latest/axum/extract/trait.FromRequest.html)

No snippets in this document have been compiled. Verify the concrete
implementation, especially nightly bounds and framework signatures.

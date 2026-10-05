# Operational decisions and entrypoints

Updated 2026-10-05. This file supersedes earlier proposals where they conflict. These are user decisions/directions unless labelled otherwise.

## Ownership and identity

- For output topics, whoever last successfully wrote owns the topic; no separate claim is needed. Input ownership is established through definition/claiming and is unaffected by value submissions; see the four-way model below.
- Identity is a name supplied by the client at connection, for example `phone tasker battery script` or `laptop battery cronjob`.
- Output writes from a different client warn by default rather than reject. External input submissions are expected and do not transfer ownership.
- Managed sessions require a client name. Read-only endpoints may operate without allocating a managed connection and without a client name. Write attribution remains needed; exact HTTP/session setup is transport-specific.
- A permissions framework is not initially necessary. A future schema could include a writer-name pattern such as `writable_by: "laptop_*"`, or access controls can be added later.
- Authentication is undecided; prioritize simplicity. Client-provided names alone are labels, not proof of identity.
- Mandatory system-namespace operation rules remain in force.

The user resolved input/output ownership through separate input definition and submission operations; see below.

Managed sessions share duplicate-name handling across APIs: warn and replace/kick an older duplicate as discussed in the latest clarification. Separate internal session IDs prevent old-session cleanup affecting its replacement. Sessionless reads do not participate in this registry.

## Persistence and events

Retained data should survive restarts through best-effort persistence; acknowledged writes need not already be durable on disk. No replay, missed-event log, or event recovery system is needed. Live connection state cannot survive restart as an active connection.

Persistence is secondary to the live state service. Storage engine, snapshot frequency, and best-effort flush behaviour remain implementation choices; strict per-write durability is not required. Reconstruct current subscription state from snapshots rather than introducing event replay.

## Expiry and freshness

Clients can send a relative expiry, e.g. one hour. The server updates last_updated and computes expires_at from its current time. Refresh expiry by writing again, even with the same value; no special refresh operation is needed.

The user's scheduling sketch uses the earliest expiring node and waits until that deadline, interrupted by notifications of expiry changes. Recompute the earliest deadline when necessary. Treat details such as indexes and timer wakeups as implementation concerns.

Accepted direction: persist an absolute timestamp, e.g. Jiff Timestamp, for last_updated and expires_at. Rust Instant is an opaque monotonic clock reading and is unsuitable as persistent or wire metadata. Monotonic timers may schedule waits, rebuilt from persisted deadlines after restart. Remove already-expired entries before including them in initial snapshots. Reference: https://doc.rust-lang.org/std/time/struct.Instant.html

Before expiry deletion, recheck the current deadline/node version so an old timer cannot delete a refreshed node. Wake scheduling for moved/deleted deadlines as well as newly inserted ones. Wall-clock adjustment behaviour remains an implementation choice.

Unspecified expiry input semantics: whether omitting expire_after on a later write preserves, clears, or renews prior expiry. Simple recommendation for discussion: preserve the existing absolute deadline unless an expiry is supplied; an explicit clear can remove it.

A schema should also support a requirement that a topic receives an update at least every specified interval. This is distinct from deletion expiry and transport connectivity. Accepted direction: report overdue status/diagnostic without deleting the node; diagnostic representation remains open. Checking this rule requires timers as well as write validation, even though the interface can stay small.

## Subscription lifecycle

Clients typically provide all subscriptions at connection setup. Examples from the user:

```rust
builder.with_subscriptions(&["/battery/*"]);
builder.with_subscriptions(&["/voice-assistant/status", "/tv/**"]);
builder.with_subscriptions(&[]);
```

One of the first server messages is a complete snapshot for the requested selection, followed by live updates. An empty selection permits a producer-only connection. Exact grouping/union semantics of repeated builder calls are not selected.

WebSocket/TCP reads are subscriptions; HTTP can support one-off reads. Initial snapshot and following updates must have no observation gap (implementation proposal consistent with intended lifecycle). Required atomic batches remain intact. Unsubscribe is not a priority initially.

Connection timeout belongs to transport implementation, e.g. keepalive or pings. No user-facing heartbeat/session-resumption framework is needed initially. Slow-consumer policy remains to be chosen; lack of replay does not imply silent dropped updates on an otherwise active connection.

## Entrypoints and shared semantics

| Entrypoint | Desired behaviour |
| --- | --- |
| Simple HTTP writes | One or multiple topic/value pairs; convenient options such as `?value=foo&expire=1h` |
| Simple HTTP reads | One-off reads of one or multiple selected topics |
| HTTP subscriptions | SSE; possibly single-topic deltas or full snapshots |
| WebSocket | Full duplex subscriptions and update batches, initial snapshot |
| TCP | Same conceptual duplex operations; transport-specific framing |
| MQTT | Later integration, low priority |

The exact API shape can differ by transport. All endpoints share core validation and state rules; every write is one or more topic/value pairs, and subscriptions use the same wildcard language. Batch HTTP operations may use the same endpoint or separate URLs. Exact URL patterns, verbs, and parameter-to-Value conversion remain open.

HTTP single-topic delivery must not silently split an atomic multi-topic batch. Accepted direction: deliver one event per atomic batch even in delta mode; full-snapshot mode emits the resulting selected state. If single-topic mode relaxes atomicity it must be an explicit documented limitation, not assumed.

Convenience strings such as `1h` may be HTTP-adapter shorthand, while the logical duration format remains ISO 8601. Decide explicitly rather than accidentally creating different core types across endpoints.

## Remaining implementation-plan questions

1. Specify input definition defaults, unclaimed input creation, and claim collision behaviour.
2. Define implicit node creation and default flags on a first ordinary write.
3. Choose omitted-expiry behaviour and input type parsing for convenient HTTP writes.
4. Choose a best-effort persistence mechanism and slow-consumer behaviour.
5. Current milestone direction: basic HTTP reads/writes first, quickly followed by WebSocket as the first fully developed transport.

These can be addressed in a short implementation plan; advanced schemas, authentication systems, and exact transport timeout tuning should not block initial implementation unnecessarily.


## Four-way ownership model — latest user clarification

| Node | Ownership | Values and lifetime |
| --- | --- | --- |
| Retained output (state) | Last accepted writer | Retain latest value; expiry applies |
| Instant output (event) | Last accepted publisher | Emit occurrence; no retained payload or payload expiry |
| Retained input (desired) | Client defining/claiming the input; claim may be optional | Other clients submit retained values without acquiring ownership |
| Instant input (command) | Same definition/claim model as retained input | Other clients submit occurrences without acquiring ownership; no payload expiry |

The owner of an input sets its metadata, including its retained/input flags. Input submissions update value provenance/last_updated independently of the owner. Separate define/claim, output publication, and input submission intents are needed, even if a transport combines them into a common envelope.

## Minimal operation sketch — assistant proposal

```rust
enum WriteOperation {
    PublishOutput { topic: Path, retained: bool, value: Value },
    DefineInput { topic: Path, retained: bool, metadata: InputMetadata },
    SubmitInput { topic: Path, value: Value },
}
```

Paths, types, and exact API names are illustrative, not selected. All operations remain compatible with atomic batches and the common core rules.

Potentially allow an unclaimed input with owner=None. A submission must never claim it. For a missing input, the caller must indicate whether it is retained or instant, or the API must choose a documented default. Definition should not silently erase an existing retained input value. These creation/default details are proposals to settle.

A defined retained input may not have received a value yet. Represent that separately from Value::Null; the latter is an actual submitted value. Instant topics may still have persistent definitions/metadata even though payloads never persist; definition persistence and cleanup remain to be specified.

Expiry of a retained input raises a separate question: expire only its desired value or remove the definition/owner as well? Assistant preference: clear the desired value while preserving the definition. This is not yet accepted and qualifies the earlier blanket node-removal idea.

Re-defining another client's input should have a deliberate rule (possibly the same warn-and-change-owner policy); ordinary submissions should not generate ownership-change warnings. Requests to change an existing input into an output or retained into instant require explicit handling, not accidental conversion by a generic write.


## Input claim expiry — user direction

Input claims have a separate lifetime from retained input values. A claim is tied to its owning session. On that session disconnecting, either relinquish the claim immediately or after a configurable grace period. This applies to both retained and instant inputs; instant payloads still have no value expiry.

Assistant representation sketch:

```rust
struct InputClaim {
    owner: ClientName,
    session: SessionId,
    release_after_disconnect: Duration, // zero means immediate release
}
```

The grace clock begins on detected disconnect, not claim creation or the last submitted input. Incoming values from other clients do not renew the claim. These follow from session-bound claim expiry, not from an added heartbeat mechanism.

Assistant proposals: releasing a claim sets owner=None while preserving the input definition and any unexpired retained value. Notify subscribers of the ownership metadata change. Cleanup of abandoned definitions remains a separate choice. Record disconnected status immediately even while the claim is held during the grace period.

A replacement claim must be protected from an old session's pending release timer. Internally associate each claim with a unique claim token/generation, or equivalent guarded comparison; a matching client name alone is insufficient. A reconnect can explicitly re-define/reclaim on its new session and replace the old claim under the chosen collision policy. Automatic same-name resumption is not selected.

Restart handling for claims and outstanding grace deadlines remains open; persisting a client name must not make an old session appear connected. This does not change ordinary retained-value persistence.


## Initial implementation scope — latest direction

Start with basic HTTP reads and writes, then quickly build WebSocket into the first fully fleshed-out transport. The common core should support both; HTTP SSE and other transports need not precede WebSocket.

Best-effort persistence is sufficient. Assistant proposal: in-memory authoritative state with periodic coherent snapshots, written through a temporary file and atomic replacement; optionally attempt a final snapshot on orderly shutdown. Report save failures through diagnostics. Crash recovery may lose writes after the last successful snapshot. Capture related state consistently so snapshots cannot restore half an atomic batch. Exact mechanism and intervals remain unselected.

Latest clarification: an API explicitly starts a managed session when relevant, then follows the common managed-connection rules, including replacement of old duplicates. Read-only requests can avoid session allocation entirely. HTTP writers may use short-lived managed sessions; exact allocation policy is still to be selected.

## Slow subscribers — explanation and proposal

A subscriber falls behind when updates are generated faster than its transport/client can receive them: a paused process, sleeping device, poor link, or broad subscription can cause this. Socket buffers absorb a finite burst, then sends cannot complete promptly. A per-client outgoing queue isolates that delay from other clients and the core.

Assistant proposal: bound each client's queued update bytes, allowing normal short delays; disconnect/report lag if the bound is exceeded. A reconnect gets a fresh retained snapshot. No event replay is added. This is basic resource bounding, not a missed-event recovery subsystem. Silently merging updates is a separate optional policy and must not collapse event occurrences or break atomic batches.


## Managed sessions versus transport connections — latest user direction

Tanuki exposes managed sessions under `/$connections/<connection_name>/...`. An adapter can start such a session when relevant; doing so subjects it to the same registration, duplicate-name replacement, session-owned claims, and disconnect cleanup rules as any other adapter. The protocol does not get its own ownership or duplicate semantics.

A read-only endpoint can perform reads without allocating a managed session or requiring a name. A physical HTTP/TCP connection is not automatically a registered Tanuki connection. An SSE adapter can still allocate internal subscription resources without a named session if read-only; this is an assistant proposal rather than a selected SSE policy.

Illustrative core interface proposal:

```rust
fn read(selector: &Selector) -> Snapshot;
fn open_session(name: ClientName) -> Session;
fn apply(session: &Session, updates: Vec<WriteOperation>) -> Result<(), Error>;
```

A sessionless caller cannot perform an operation that requires an owning session. One-off HTTP writes can use a short-lived named session; retained output values survive its close, while any input claims follow their disconnect expiry rules. This is a proposed simple write mapping, not a new requirement that every request register a session.

The same-name registry entry must be guarded by internal session identity on replacement, unregister, and timer cleanup. Old sessions lose mutation authority once displaced. Definition of connection-name path escaping and which session metadata is exposed remains open.

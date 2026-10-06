# Tanuki — proposed implementation plan

2026-10-05. This is a proposed sequence, not approval of every default in the
decision review. Read [spec.md](spec.md) and
[decisions-to-review.md](decisions-to-review.md) first. Preserve the
distinction between agreed behaviour and recommendations.

## How to execute this plan

Follow [AGENTS.md](../AGENTS.md). Review the proposed interfaces in
[design.md](design.md) and use [test-plan.md](test-plan.md) as the
behavioural contract. The numbered task cards below make the phases executable;
do not attempt all phases in a single unreviewed change.

Each card follows red → green → refactor, with evidence of the relevant tests.
Decisions marked proposed remain proposed until selected and recorded. The user
has authorized planning, not implementation in this conversation; this handoff
is intended for the implementing agent.

## Implementation principles

Keep it simple and fun to hack on. Robust operation should be possible without
turning every edge case into a subsystem. Log unusual behaviour liberally and
prefer warnings for recoverable misuse unless a denying schema requires
rejection. Preserve basic core invariants and built-in system rules. Treat the
review as a set of small policy decisions, not a mandate to build every
proposed restriction.

## Approach

Build a small transport-independent core, expose useful HTTP operations, then
add WebSocket early. Do not build a general scripting runtime, entity
framework, blob service, or complete schema language before this works.

Rust nightly is selected. Use Tokio and the Axum web stack, subject to checking
concrete dependency APIs. Jiff is the user's suggested time library. Choose
dependency versions during implementation; this document does not freeze
libraries or versions.

Keep one authoritative commit path for HTTP, WebSocket, timers, and later
adapters. A serialized core task is a reasonable first implementation; network
sends and disk I/O must not block it. Separate logical types from transport
encodings. Do not make the persistent snapshot an undocumented serialization of
every internal struct.

## Phase 0 — Record a minimal executable contract

Apply the accepted input lifetimes in D1 and sessionless HTTP writes in D2;
choose the remaining small lifecycle defaults and path rules in D7. Select the
initial runtime/codec profile in D5 and transaction rules in D6 before
publishing external protocol examples. D8 defaults can be selected as
implementation choices and documented.

Write a small protocol document containing actual request/reply examples for
publication, input definition/submission, atomic batches, snapshot,
removal/value clearing, and errors. Specify protocol and persistence format
versions. Keep these examples subordinate to the agreed semantics, not an
opportunity to add requirements.

Do not block this phase on final advanced schema syntax or link recovery
policy; settle D3/D4 before those features ship.

**Exit condition:** the agent can answer what each accepted write
creates/changes, how each kind expires, which session it belongs to, and how a
client observes the result. Any provisional policy is visibly listed.

## Phase 1 — Core and basic HTTP

Implement:

- Canonical paths and the common selector matcher.
- Runtime values and metadata, with definition/payload/claim represented
  separately.
- Managed session registration, duplicate replacement, and internal session
  identity.
- Four node kinds and distinct publish/define/submit intents.
- One atomic batch application path with structured results and warnings.
- One-off snapshot reads and minimal HTTP single/batch reads and writes.
- Reserved namespace routing and enough built-in validation to expose
  connection records safely.

Do not add full schemas and links just to make a first HTTP battery script
work. Keep ordinary validation and system operation checks separate so those
features can be added without moving validation into adapters.

**Meaningful checks:** input submissions do not transfer ownership; output
replacement warns; a rejected batch changes nothing; duplicate writes refresh
metadata; maps stay single values; displaced session cleanup cannot remove its
replacement; anonymous reads and one-off writes create no managed session; a
same-name one-off write cannot displace a persistent client.

**Usable result:** publish two battery values and query `/battery/*` through
HTTP, using an agreed expiry policy once Phase 3 lands.

## Phase 2 — WebSocket and coherent subscriptions

Add JSON text and MessagePack binary messages using the same core operations.
Implement startup selection, complete snapshot, update batches, explicit
removals/value clearing, event occurrences, and request-correlated replies.
Unsubscribe is optional later.

Establish subscription and snapshot at a single core boundary. Use bounded
outgoing queues with the selected slow-client policy. Preserve one commit as an
indivisible application update; apply all changes before exposing a recomputed
client shape.

Build a minimal client/example that maintains a selected local view. A polished
serde wrapper API can follow; prove value/metadata/input-writing semantics
first.

**Meaningful checks:** write concurrent with subscription startup is neither
lost nor represented as a partial batch; instant events do not appear as
replayed snapshot values; overlapping selectors do not duplicate a topic;
paired hue/brightness updates remain atomic; a slow client cannot stall
unrelated clients; codecs round-trip semantic values and literal tag-looking
maps.

**Usable result:** a live battery widget, plus a dashboard combining arbitrary
paths, updates from a complete current view.

## Phase 3 — Lifetimes and best-effort restart

Implement value expiry, separate claim release timers, and generation/deadline
guards. Expose metadata changes to subscribers. Apply the selected policies for
omitted expiry, disconnect grace, and restart claims.

Add best-effort persistence using the selected mechanism. Restore retained data
and definitions coherently, omit expired payloads, and never resurrect sessions
as live. Keep network acknowledgement independent of disk completion.

**Meaningful checks:** refreshing before an old deadline prevents deletion;
reclaiming prevents an old release timer from clearing the new claim; input
payload expiry preserves its definition and claim; restart does not expose
expired data; a snapshot cannot contain half a committed batch; a failed save
is visible.

**Usable result:** long-lived laptop state, periodic phone updates, and desired
inputs retain their intended meaning across disconnects and restarts.

## Phase 4 — Schema boundary and initial validators

Resolve D3, then implement atomic installation and deterministic matching.
Start with a small useful validator set: primitive kinds, optional/null policy,
numeric ranges, and simple enum constraints. Add richer records/collections
only as examples require them.

Rejecting validation runs before commit and event delivery. Keep direct casting
separate from validation and ensure final results satisfy all rejecting rules.
A schema warning allows and logs the value; a schema error denies it. Built-in
system operations never pass through user schema enforcement.

Add overdue-update diagnostics as a timed rule when freshness configuration is
supported. This must preserve the value and remain independent of expiry and
connection status.

**Meaningful checks:** an invalid direct value cannot reach subscribers under
rejecting rules; failed atomic schema installation changes nothing; cast
results are revalidated; warning mode is distinguishable; explicit system
targeting is rejected; a broad selector follows the agreed system-domain rule.

**Usable result:** battery values constrained to a chosen representation/range,
with invalid writes explained to their caller and overdue data observable.

## Phase 5 — Linked views

Resolve D4 before implementing links. Build forward and reverse indexes with
subtree-aware propagation. Check cycles, collisions, and the system boundary
according to the selected profile.

Compute source changes and all linked effects as one commit. Enforce linked
schemas without casting the source. A rejecting violation disables the entire
offending view, with explicit diagnostics and removals of previously visible
data. Automatically re-enable when valid again and log recovery. Support writes
through links under the selected validation rules. Persist link definitions
according to the selected restart policy.

**Meaningful checks:** a descendant change reaches aliases; one invalid
descendant affects its whole mounted view; valid source data still commits; no
invalid value leaks in removal metadata; an invalid event is handled according
to the selected rule; restart rebuilds indexes; link replacement/deletion does
not leave stale subscriptions; repaired data automatically restores a disabled
view; alias writes follow the chosen validation rules.

**Usable result:** two coherent views of the same smart-home data, including a
schema-governed view.

## Initial implementation completion

Run a compact end-to-end scenario combining:

1. One persistent laptop publisher and one periodic phone publisher.
2. A dashboard subscribing to battery and TV paths.
3. An input owner receiving a desired value or command from a different client.
4. An atomic multi-property update.
5. Expiry, disconnect/replacement, restart, and a rejected schema write.
6. A linked view invalidated without rejecting its valid source write.

The implementation handoff should include a runnable server, HTTP and WebSocket
examples, exact selected defaults, local diagnostics, and a list of remaining
unsupported features. Verify behavioural boundaries; avoid tests that merely
repeat accessor implementations.

## Explicitly later

- TCP framing and MQTT integration.
- SSE and server-generated full-shape delivery unless a concrete early client
needs them.
- Advanced schema DSL features and complex cross-topic validation.
- Full permission/authentication frameworks; initial deployment assumptions
still need documentation.
- Peer-to-peer transfer negotiation, streaming, and large-blob infrastructure.
- Embedded script execution and runtime supervision.
- Sophisticated client-library derives and automatic input/output binding.

No event replay, exactly-once command delivery, or per-write disk durability is
planned. These are not hidden requirements for finishing the initial service.


## Task cards

These cards are sequential unless dependencies say otherwise. Each delivery
includes focused test results, updated relevant docs, and a short report. Build
the test harness with the first behaviour it verifies; do not postpone testing
until the final task.

The 2026-10-06 [client API proposal](client-api-design.md) is a separate design
checkpoint for reusable client crates. The user selected WebSocket-only scope,
protocol-level primitives with channel-based observation helpers, and latest
complete snapshot delivery for observers. Typed payloads and Rust-first delivery
with a future browser WASM path remain the direction; cross-topic Serde shapes
come later.
Iced is the primary consumer use case, without becoming an SDK dependency.
The [client SDK handoff](client-api-handoff.md) records decisions, provisional
defaults and bounded implementation tasks. This conversation prepared the
handoff; implementation starts when the user resumes it with the chosen model.

### 00 — Bootstrap and architecture checkpoint A

**Dependencies:** none. **Deliver:** pinned nightly, library plus thin binary
skeleton, rustfmt/Clippy/test commands and CI, initial docs/toolchain.md and
module map. Verify chosen crate/toolchain compatibility before committing
versions.

Present node, identity, operation and core API sketches from `design.md`
plus C1/C2 test examples to the user. Highlight only remaining decisions that
affect representation: operation creation/kind rules, session-bound claim
access, and initial value profile. Record simple provisional defaults for
routine details. Do not build domain modules around an unreviewed incompatible
representation.

**Check:** compile/lint baseline; a minimal test harness can start and stop
cleanly. **Exclude:** transport implementation, schema DSL, link indexes.

### 01 — Domain values, identities, paths and selectors

**Dependencies:** 00 review. **Deliver:** private invariant-bearing newtypes,
node representation, operation types, structured parse errors. Implement the
agreed initial selector grammar with a documented test table. Define runtime
values independently of transport tagging.

**Red tests:** U1 missing/null distinction, map isolation and path/matcher
boundaries. Test external conversion cannot bypass constructors. Use
compile-fail documentation selectively for a meaningful API restriction, not
for every private field.

**Done:** state representation admits all supported forms and excludes
impossible payload/kind combinations. **Exclude:** guessed future schema types
and large-blob machinery.

### 02 — Core publication and atomic batches

**Dependencies:** 01. **Deliver:** deterministic in-memory read/apply
operations, explicit clock input, retained outputs and instant occurrences,
warning diagnostics, output provenance and atomic staging. Keep the mutation
API ready for later validators without building a generic plugin pipeline.

**Red tests:** C2 partial-failure rollback using an actual invalid operation;
duplicate writes refresh metadata; output takeover warns; instant occurrences
do not enter retained snapshots. Use an in-memory observer of commit batches
until subscriptions land.

**Done:** one mutation path owns state changes; errors leave state unchanged.
**Exclude:** disk durability and networking.

### 03 — Managed sessions and input operations

**Dependencies:** 02. **Deliver:** session registry, duplicate replacement,
invalidated old handles, input definition/claim/submission, explicit stateless
context. Disconnect records claim release work for the later timer task;
immediate release can work now.

**Red tests:** C1 in full; submit-before-owner; claim preserves value; command
with no owner is allowed; same-name stateless context cannot impersonate a
managed handle. Define how a stateless operation creates an unclaimed input
rather than acquiring a session claim.

**Done:** owner, submitter and session are distinct in both code and tests.
**Exclude:** authentication framework and session resumption.

### 04 — Basic HTTP vertical slice

**Dependencies:** 03 and initial codec profile selected from D5. **Deliver:**
Axum router, read and single/batch write endpoints, JSON codec, typed request
extraction, ApiResponse/ApiError mapping, structured logs and minimal server
configuration.

**Red tests:** router integration for malformed payload, missing attribution
and domain failures all using the common error format; actual HTTP producer and
reader in the E1 harness; one-off writes leave the managed registry unchanged.
Store/read binary and semantic values using the selected JSON representation.

**Done:** run a battery publisher and read its state through the real server.
**Exclude:** SSE, advanced schemas and endpoint-specific core logic.

### 05 — Subscription contract and checkpoint B

**Dependencies:** 04. **Deliver:** atomic subscribe/snapshot boundary,
selection filtering, batch delivery and bounded queues. Review
snapshot/update/error DTOs with the user before freezing wire behaviour. Log
and document the chosen slow-consumer and repeated-target policies.

**Red tests:** C3 controlled registration races, explicit empty snapshot,
metadata-only changes, input payload clearing and atomic batch projection.
Queue overflow must not block the core.

**Done:** consumers can maintain a coherent selected view without knowing
internal storage. **Exclude:** whole-shape server rendering, unsubscribe and
replay.

### 06 — WebSocket and mixed-client simulation

**Dependencies:** 05. **Deliver:** managed WebSocket lifecycle, JSON
text/MessagePack binary codecs, shared request/reply semantics, minimal mock
client helpers for both production and consumption.

**Red tests:** T1 transport parity and codec vectors, real socket
closure/session cleanup, and E1 steps 1–4. Assert downstream outputs, not just
producer acknowledgements. Document snapshot-versus-reply ordering; do not make
clients guess whether the first message is an acknowledgement.

**Done:** phone HTTP, laptop WebSocket, dashboard and lamp/motion scripts
interact through the server. **Exclude:** polished public SDK, embedded script
runtime, TCP/MQTT.

### 07 — Expiry and input-claim grace

**Dependencies:** 06. **Deliver:** earliest-deadline scheduling, wakeups and
guarded expiry, metadata notifications, deterministic clock harness. Select and
record omitted-expiry behaviour.

**Red tests:** L1/L2 including stale timers, submission during grace and
reclaim before old release; E1 step 5 over transports with controlled time.
Distinguish value clearing from node removal.

**Done:** timer actions use the same core consistency boundary as writes.
**Exclude:** wall-clock synchronization service or distributed leases.

### 08 — Best-effort persistence

**Dependencies:** 07. **Deliver:** versioned coherent snapshots, async file
saving outside core transitions, restore and shutdown procedures, observable
save failure. Select snapshot interval and corrupt-file policy explicitly.

**Red tests:** P1 with isolated storage and injected failures, plus a
subprocess restart smoke test. Release correctness must not rely on validation
performed only in debug assertions.

**Done:** acknowledged live writes remain independent of disk completion;
restored claims are absent. **Exclude:** WAL, replay and transactional storage
engine unless a demonstrated need changes scope.

### 09 — Schema policy and checkpoint C, part one

**Dependencies:** 08. **Deliver:** review the small schema interface, rule
configuration, cast/error/warning outcomes, overlap and installation
procedures. Implement the initial validators and mandatory system namespace
separation.

**Red tests:** S1 warning-versus-denial, atomic activation failure, casts and
attempts through every existing write adapter. Add feasible pure debug checks
downstream of primary validation. Tests also pass in release mode.

**Done:** permissive freeform behaviour stays intact; denying schemas cannot be
bypassed by another adapter. **Exclude:** broad cross-topic query language or
Rust types generated for user schemas.

### 10 — Writable links and checkpoint C, part two

**Dependencies:** 09. **Deliver:** agree alias-write validation and
disabled-alias repair semantics; implement forward/reverse subtree indexes,
visibility, disabling and automatic recovery. Decide instant-occurrence
recovery using one concrete example, without adding history.

**Red tests:** K1 and alias collision/cycle policy; direct valid writes survive
rejecting view violations; all projected changes are atomic. Extend E1 step 6.
Persist definitions and rebuild indexes on restore.

**Done:** repairing data or applicable policy can restore a disabled link
automatically. **Exclude:** read-only shortcuts that contradict the accepted
writable-link requirement.

### 11 — Freshness and shared diagnostics

**Dependencies:** 09; can precede 10. **Deliver:** expected-update-interval
checks, observable overdue status, and core diagnostics exposed through the
selected system interface. Local logging must work throughout earlier tasks;
this task adds shared observation, not the first logs.

**Red tests:** overdue status preserves data, a duplicate accepted write
restores freshness, warning/recovery transitions are observable, and a
diagnostic publication failure cannot recursively flood the system. No embedded
runtime is required.

**Done:** users can inspect unusual behaviour centrally and locally.
**Exclude:** external notification integrations; those can be ordinary client
scripts.

### 12 — Initial release review

**Dependencies:** 00–11. **Deliver:** complete E1, protocol examples,
module/data-flow/procedure docs, selected defaults and supported/deferred
feature list. Run formatting, Clippy, unit/integration/end-to-end suites and
release-mode correctness checks on the pinned toolchain.

Review the resulting public/local APIs with the user, including any deviations
from earlier checkpoints. Confirm there are no speculative frameworks, ignored
acceptance tests, or schema validation hidden only in HTTP. Fix concrete
failures, then stop broadening the initial scope.

**Done:** another agent or user can run the server, run the system simulations,
inspect diagnostics and understand the mutation path from the documentation.

### Native SDK follow-through — 2026-10-07

SDK-01 through SDK-06 from [the client handoff](client-api-handoff.md) are
implemented for the native plain-WebSocket profile. Shared protocol and SDK
packages, direct wire control, background sessions, weak typed handles, atomic
writes and raw/latest-state observation now have compiled implementations and
real-server acceptance tests. Consumer examples were exercised together against
the production binary. See [testing.md](testing.md) for task/story coverage and
[the SDK guide](../crates/tanuki-client/README.md) for exact APIs/defaults.
Browser transport/bindings and Iced-specific integration remain out of scope.
The protocol-only browser check could not run because target std is absent;
TLS and Serde 128-bit methods remain explicit native limitations.

# Protocol

Protocol version 1 exposes JSON over HTTP and JSON text or MessagePack binary
messages over WebSocket. The representation remains provisional until
checkpoint B reviews snapshot/update/error DTOs; incompatible changes are
allowed before that checkpoint.

## Current operation boundary

A nonempty `WriteBatch` is attributed through a `WriteContext` and applied at
an explicit server timestamp. One accepted batch produces one ordered,
sequence-numbered `UpdateBatch` plus zero or more warnings. A rejected batch
has no node, occurrence, or sequence effects.

State publications retain their latest complete runtime value and provenance.
Event publications update publisher metadata and emit an occurrence in the
current batch; occurrence payloads never appear in snapshots. A snapshot is a
selection-filtered map of current nodes at one commit sequence.

Input definitions exist independently of claims and payloads. Desired
submissions retain their latest value and submitter provenance without changing
the claim. Command submissions emit an occurrence and retain no payload.
Claiming requires a current managed session, but definition and submission may
use an attributed stateless context. A batch may atomically define, claim, and
submit on one input topic in operation order.

The current core executes repeated same-topic operations sequentially against
one private candidate and rejects writes to the reserved `$` namespace. The
observable update coalesces retained mutations to the final node shape while
preserving every event and command occurrence in operation order. Thus a
create followed by removal can commit successfully with no retained change
visible to subscribers. The persistence format is internal and not a durable
API.

## HTTP version 1

Writes require a validated client name supplied by either the `tanuki-client`
header or `client` query parameter. If both are present they must agree. This is
attribution, not authentication, and creates no managed session. Reads are
anonymous. The header deliberately has no `X-` prefix; that convention is
deprecated by RFC 6648.

Reference: [RFC 6648](https://www.rfc-editor.org/rfc/rfc6648) for the `X-`
prefix.

| Method and path | Body/query | Meaning |
| --- | --- | --- |
| `POST /v1/state/{topic...}` | `{"value":72,"expiry":{"mode":"clear"}}` | Convenience retained-state write |
| `POST /v1/write` | `{"operations":[...]}` | One atomic operation batch |
| `GET /v1/snapshot?select=/battery/*` | one or more `select` | Selection-filtered retained snapshot |
| `GET /v1/sse?select=/battery/*&select=/lamp/**` | one or more `select` | Snapshot followed by live atomic update events |
| `PUT /v1/schemas/{name}` | complete schema declaration | Atomically install or replace one named schema |
| `PUT /v1/links/{name}` | `{"mount":"/view","target":"/canonical"}` | Atomically install or replace a writable subtree link |
| `DELETE /v1/links/{name}` | none | Remove a link and retract its projected nodes |

Batch operations use an `op` discriminator: `publish_state`, `publish_event`,
`define_input`, `claim_input`, `submit_desired`, `submit_command`,
`clear_desired`, and `remove_node`. Retained writes accept an expiry object:
`{"mode":"preserve"}`, `{"mode":"clear"}`, or
`{"mode":"set","duration":"PT1H"}`. Omission means `clear`: a writer that
says nothing about expiry never inherits a deadline another writer chose.
`preserve` explicitly keeps the existing absolute deadline (none for a new
value). Only a deadline of the same node kind is kept: publishing state over a
desired node discards the desired payload and its deadline together. Claim releases similarly use `immediate` or `after` with a nonnegative
fixed ISO-8601 duration.

Each value's JSON encoding, measured after any schema or alias cast, is
limited to 256 KiB (`tanuki::core::MAX_VALUE_JSON_BYTES`). JSON is the larger
codec, so the bound also covers MessagePack, and a single value always fits a
1 MiB WebSocket message. A larger value rejects the whole batch with
`value_too_large` (HTTP 413).

Successes are `{"ok":true,"data":...}`. A write's `data` is a commit:
`{"sequence":43,"warnings":[]}` (`tanuki_protocol::CommitView`), the same
object a WebSocket `reply` carries as `result`. Reads repeat `select` for
several selectors, like a WebSocket hello's list; a missing or invalid
selector is `invalid_query`.

### Error codes

HTTP error bodies and WebSocket `error` messages share one set of codes
(`tanuki_protocol::ErrorCode`). A code unknown to an older client decodes as
`ErrorCode::Unknown`; the message still explains it.

| Code | HTTP | Meaning |
| --- | --- | --- |
| `already_initialized` | — | A second WebSocket `hello` |
| `hello_required` | — | The first WebSocket message was not `hello` |
| `invalid_message` | — | A WebSocket message could not be decoded |
| `invalid_json`, `invalid_query` | 400 | Malformed HTTP body or query |
| `missing_client`, `invalid_client`, `conflicting_client` | 400 | HTTP write attribution absent, invalid, or contradictory |
| `invalid_topic`, `invalid_duration`, `negative_duration`, `empty_batch` | 400 | Invalid request fields |
| `invalid_schema`, `invalid_link` | 400/409 | Invalid schema or link declaration or topology |
| `deadline_out_of_range` | 400 | Expiry or grace beyond the timestamp range |
| `session_required` | 400 | Claiming needs a managed session (HTTP and stateless WebSocket cannot claim) |
| `system_topic` | 403 | Writes to `/$...` are forbidden |
| `input_not_defined` | 404 | Submission/claim/clear on an undefined input |
| `not_found`, `method_not_allowed` | 404/405 | Unknown endpoint or method |
| `session_expired` | 409 | The managed session was replaced or disconnected |
| `claim_authority_required` | 409 | Another session's claim controls the definition |
| `wrong_node_kind` | 409 | The operation does not fit the existing node kind |
| `schema_overlap`, `existing_schema_violations` | 409 | Schema installation conflicts |
| `value_too_large` | 413 | A value's JSON encoding exceeds 256 KiB |
| `schema_violation` | 422 | A denying schema rejected the batch |
| `capacity_exhausted`, `internal_error` | 500 | Server-side failure |
| `server_shutdown` | 503 | Shutdown has closed admission | Every adapter, extractor, routing,
and core failure uses `{"ok":false,"error":{"code":"...","message":"..."}}`
with an appropriate HTTP status. A successful write returns its commit sequence
and structured warning list; it does not promise command execution or durable
storage.

Schema installation accepts `{"force":false,"rules":[...]}`. Each rule has a
`selector`, `enforcement` (`warn` or `deny`), optional `nullable`, optional
`node_kind`, `expected_update_interval`, a tagged `validator`, and optional explicit cast. Initial validator types are `any`,
`kind`, `integer_range`, `float_range`, and `string_enum`; initial casts are
`string_to_integer`, `string_to_float`, and `string_to_bool`. The default mode
rejects current deny violations. `force:true` removes deny-invalid state and
clears deny-invalid desired current values while preserving definitions and
claims. Warning violations remain and are returned as diagnostics. A complete
declaration atomically replaces the same schema name.
Like other HTTP mutations, schema installation requires client attribution by
`tanuki-client` or `?client=`; this is not authentication or authorization.

Link installation and deletion require the same attribution. A link projects
the target subtree beneath its mount in snapshots and update batches. Writes to
an alias are translated to the canonical topic after alias schema validation;
the canonical schema is then applied. Link changes report a commit sequence
when visible aliases were added or removed. Deleting an absent link is an
idempotent success.

`expected_update_interval` is an optional positive fixed ISO-8601 duration such
as `"PT5M"`. It applies to retained state and present desired values matched by
the rule. The shortest matching interval is effective.

## Read-only diagnostics

Active conditions appear as state nodes and ordinary atomic updates beneath
`/$diagnostics/**`. Current paths are:

- `/$diagnostics/freshness/<source...>` for retained values whose expected
  update interval elapsed;
- `/$diagnostics/links/<link-name>` for schema-disabled linked views.

The value is a map containing a stable `code` plus relevant context such as the
source topic, interval, last update, link name, and violation message. Onset is
an upsert; recovery is a removal. These paths are Tanuki-owned: clients may
select them but cannot write them, schemas do not apply, and snapshots derive
them instead of persisting them.

Ordinary JSON nulls, booleans, signed safe integers, finite numbers, strings,
arrays, and objects map directly to runtime values. The tagged forms are:

```json
{"$bytes":"AAEC/w=="}
{"$int":"9223372036854775807"}
{"$map":{"$bytes":"literal, not a tag"}}
```

Integers outside JavaScript's exact range encode with `$int`; unsigned values
outside signed 64-bit range are rejected. A literal map containing any reserved
tag key must use `$map`. Encoding is recursive and lossless for the accepted
initial value profile. There are no timestamp or duration values: dates and
intervals inside payloads are strings or integers. `$timestamp` and `$duration`
are not reserved and form ordinary maps.

The shared JSON dependency enables exact float round-trip parsing, preserving
finite f64 bits (including subnormals and signed zero) across the wire.
MessagePack array lengths are untrusted: runtime lists allocate as elements
decode, rather than reserving memory for a potentially truncated array header.
These checks do not establish a total decoded-memory budget.

Input nodes (desired and command) carry their `claim`; the former empty
`"definition": {}` field was removed on 2026-11-22. State and desired nodes
use an explicit `current` wrapper. Desired
`"current":null` means no payload exists; `"current":{"value":null,...}`
means `Value::Null` was actually submitted. This distinction is preserved in
snapshots and updates.

## Subscription messages

The accepted stream uses a correlated snapshot as successful subscription
acknowledgement:

```json
{"type":"snapshot","request_id":"s1","sequence":42,"nodes":{},"warnings":[]}
{"type":"update","sequence":43,"changes":[]}
{"type":"reply","request_id":"w1","result":{"sequence":43,"warnings":[]}}
{"type":"error","request_id":"w1","error":{"code":"...","message":"..."}}
```

Changes are tagged `upsert`, `event`, `command`, or `removed`. Upsert carries a
complete node view; removal carries the previous complete node. Consumers apply
the entire changes array before exposing a recomputed shape. Instant
occurrences are returned separately by the minimal client view and never enter
its retained node map.

The sequence is the global in-memory commit sequence, not a replay cursor.
Filtered subscriptions legitimately skip commits that affect no selected
topic, so a numeric gap alone does not prove message loss. Reconnection starts
from a new snapshot; replay is not promised.

## Server-sent events version 1

`GET /v1/sse?select=<selector>` opens an anonymous read-only subscription. Its
first event is always `snapshot`; later `update` events each contain one
complete selected commit batch. The SSE `data` field is the JSON encoding of
`SnapshotView` or `UpdateView` respectively, without the WebSocket message
type or request ID. The SSE `id` is the same global sequence carried in the
JSON body:

```text
event: snapshot
id: 42
data: {"sequence":42,"nodes":{}}

event: update
id: 43
data: {"sequence":43,"changes":[]}
```

The server emits a keep-alive comment after 15 seconds without data. It does
not interpret `Last-Event-ID`: reconnecting creates an ordinary new
subscription and sends its current snapshot, with no missed-event replay. A
subscriber that exhausts the core's 64-batch queue is disconnected; it does
not block writers or split/coalesce a batch.

## WebSocket version 1

`GET /v1/ws` upgrades to a managed full-duplex session. The first data message
must be `hello`; its frame kind selects the codec for unsolicited server
snapshots and updates:

```json
{"type":"hello","request_id":"h1","client":"laptop","selectors":["/battery/*"]}
```

Every text frame contains JSON and every binary frame contains MessagePack,
including when a client mixes them on one connection. A request-correlated
reply uses the request frame's codec. A successful
hello receives exactly one correlated `snapshot` before ordinary replies or
updates. The snapshot's `warnings` include managed-session replacement.

Hello's optional `session` field chooses what the connection establishes
besides its subscription:

- `"managed"` (the default when omitted) opens a managed session. It can claim
  inputs, and a later managed hello with the same client name replaces it.
- `"stateless"` attributes writes to the client name exactly like a stateless
  HTTP request. It cannot claim inputs and neither replaces nor is replaced by
  any other connection, so dashboards, inspectors and browser tabs can share a
  name. Its write provenance carries no session.

Later client messages are `write` messages whose `operations` array is
identical to the HTTP batch operation array:

```json
{"type":"write","request_id":"w1","operations":[{"op":"publish_state","topic":"/battery/laptop","value":87,"expiry":{"mode":"clear"}}]}
```

or `select` messages, which replace the connection's selection:

```json
{"type":"select","request_id":"s2","selectors":["/battery/*","/lamp/**"]}
```

A `select` is answered by a correlated `snapshot` of the new selection (with
no warnings). Updates queued for the old selection are discarded because the
new snapshot is taken after all of them; later updates follow the new
selection only. An empty list is valid and stops all updates.

A data message must contain exactly one encoded protocol message. Trailing
JSON or MessagePack data rejects the whole message before applying a write.
The shared `decode_messagepack` helper enforces this for the server and the
Rust connection.

A write's own selected update, and any update queued before it, is sent
before its reply. When the reply arrives, a client that applies updates in
order therefore already reflects its own write (read-your-writes), provided
the write touched its selection. Updates from unrelated concurrent writers can
also be queued, so clients distinguish correlated replies by `request_id`
rather than assuming every adjacent message is a pair. Decode failures whose ID cannot be recovered
use `request_id: null` and the malformed frame's codec.

Opening another managed WebSocket with the same client replaces the old
session. The old socket closes with code 4001 and reason `session_replaced`;
same-name stateless HTTP traffic does not replace it. Normal disconnect runs
claim cleanup. A slow core subscription closes with code 1013 and reason
`slow_consumer`; reconnecting starts from a fresh snapshot.

Liveness uses ordinary WebSocket mechanisms (provisional defaults):

| Condition | Default | Close |
| --- | --- | --- |
| No hello after the upgrade | 10 s | 1008 `hello_timeout` |
| Server ping interval once open | 20 s | — |
| Peer sends nothing at all (data, ping or pong) | 60 s | 1011 `peer_timeout` |
| One outgoing write does not complete | 10 s | connection dropped |
| An encoded outgoing message exceeds 1 MiB | — | 1009 `message_too_big` |

Peers answer pings automatically while their WebSocket library reads the
stream. A `tanuki_client::Connection` user that stops calling `recv` stops
answering and is disconnected after the silence window. The server acknowledges
a client's close frame before dropping the connection.
The server answers peer pings itself and never sends a duplicate pong. A
silence or write timeout runs ordinary disconnect cleanup, so a controller
that vanishes without closing TCP releases its claims (or starts their grace).

Inbound frames and individually encoded outbound messages are limited to 1
MiB. The per-value limit keeps a single value deliverable, but a snapshot or
update combining several large values can still exceed 1 MiB; that connection
then closes with 1009 instead of vanishing silently (see `open-questions.md`).
Each subscription currently buffers at most 64 complete commit batches; the
open byte-budget decision is tracked in `open-questions.md`.

MessagePack uses native primitives and binary values and no extension types;
a value containing an extension type (including the standard timestamp type
`-1`) is rejected.

## Persistence format 3

The local snapshot is MessagePack with `format: "tanuki-snapshot"` and
`version: 3`. It is an internal restart format, not a client transport or an
acknowledgement log. It contains one coherent commit sequence and retained
node/definition metadata, installed schemas and link definitions, but no live sessions, claims,
event occurrences, or command occurrences. Future incompatible formats must use a new version;
unknown versions are treated as invalid snapshots. Timestamp and duration
values were removed from the value model without a version change; a version 3
snapshot that still contains one, or a schema requiring one, is invalid. At startup invalid data is
copied beside the configured file with a `.bak` suffix (or `.bak.N` without
overwriting an earlier backup), a warning is logged, and Tanuki starts with
empty state. Ordinary read or backup I/O failures still fail startup.

## Native Rust SDK usage

`tanuki-protocol` supplies bidirectional v1 requests/messages and lossless value
codecs. View DTOs carry validated types: topics are `TopicPath`, writers and
claim owners `ClientName`, times `jiff::Timestamp`, claim grace
`jiff::SignedDuration`, node kinds `NodeKind`, and error codes `ErrorCode`.
Their JSON and MessagePack text is unchanged; invalid paths or names are
rejected while decoding. `WireValue` (formerly `JsonValue`) is the
codec-aware value wrapper, and `to_value`/`from_value` plus the
`decode::<T>()` helpers on `WireValue`, `CurrentValueView` and
`OccurrenceView` convert ordinary Serde types. The server supplies conversion functions `snapshot_message` and
`update_message` for authoritative core data. Payload-bearing tagged enum
decoding keeps the original JSON/MessagePack decoder through nested fields;
bytes, extension values and literal reserved-key maps now round-trip through
whole requests, snapshots and updates, not just standalone value fixtures.

`tanuki-client` is the protocol layer: `Connection` sends typed client
messages and receives typed server messages over one WebSocket, natively
(`ws://`, and `wss://` through the default `rustls` feature) or in the browser,
and splits into a cloneable `Sender` and a `Receiver`. Hello, request IDs,
caching and reconnection belong to the application. See
[the client README](../tanuki-client/README.md).

## Server boundary and shutdown — 2026-10-07

HTTP and WebSocket request conversion shares `protocol::RequestConversionError`
and the same validated `WriteBatch` conversion. Typed commit and management
response DTOs reuse `DiagnosticView`; HTTP envelopes and the existing generic
WebSocket reply result retain their version-1 JSON shape. No wire or snapshot
version change is required by this refactor.

Once shutdown closes admission, a request that reaches a still-draining adapter
is rejected with the common `server_shutdown` code (HTTP 503). Already accepted
writes remain committed even if shutdown prevents delivery of their reply.
WebSockets attempt close code 1001 with reason `server_shutdown`; a forced
socket close may instead appear as EOF or transport failure. HTTP/SSE draining
and WebSocket close delivery have a provisional one-second window, after which
remaining socket I/O is cancelled. Reconnect obtains a new snapshot as usual.

Link management rejects names `.` and `..` with `invalid_link`; other existing
valid punctuation, spaces, and Unicode names remain supported. Restore rejects
reserved canonical nodes and denying policy violations before exposing state;
warning policies still accept without casting stored data.

## Inspector foundation consumer — Task 01

The headless `tanuki-ui` model consumes existing `SnapshotView`/`UpdateView` and
`NodeView` DTOs through `SelectedView`; it introduces no endpoint or wire change.
It accepts increasing sequence gaps, applies complete batches, and keeps instant
occurrences out of retained state. Invalid topic strings are rejected before
cache mutation. Schema/link enumeration and explanation are still unavailable;
the detail formatter explicitly labels schema information unavailable rather
than claiming no schema applies. Live subscription/system-namespace scope will
be verified through real transport in Task 02.

## Inspector live consumer — Task 02

The inspector subscribes to `/**` (since 2026-11-22 through a raw
`Connection` with a stateless hello). Real-server tests confirm this subscription includes
exposed `/$diagnostics/**` as well as ordinary topics, both in initial snapshots
and later updates. Schema matchers' ordinary-only restriction does not apply
to subscription matching. No `/$connections/**` projection currently exists;
the observer is a managed session but cannot be shown as a connection node.
No new protocol surface is added to fill that gap in this task.

Listener registration's baseline replaces the cache on explicit reconnect;
hello is not reused as that baseline. Complete batches are consumed in order;
raw lag is terminal and marks cached data stale. Clean shutdown and transport
reset are distinct SDK outcomes, both non-live. Reconnect is a new session and
snapshot, with no retry/replay promise. Explicit duplicate names preserve
server replacement semantics; generated local defaults avoid ordinary local
collisions. Session close joins client tasks, not server-side cleanup delivery.

## Inspector milestone 1 terminal — Task 03

`tanuki-ui` now exposes the existing node inspection surface through a read-only
terminal application, with configurable WebSocket URL/client name. It still
uses only the SDK `/**` snapshot/update stream, preserves complete batches and
separates instant metadata from occurrences. A bounded connection job changes
UI ownership, not wire behavior: explicit reconnect joins the old session and
registers a new listener/baseline. Connecting/disconnected/lagged/error data is
labeled stale; there is no retry or replay.

No schema/link enumeration or explanation endpoint is introduced. The terminal
says schema information is unavailable and still cannot fabricate the missing
connection-topic projection. Public inspection API design is checkpoint B,
Task 04. Milestone 1 node inspection is not complete public-state coverage.

## Browser Rust SDK — 2026-10-31

The same Rust `Connection`, `Session`, typed write and observation APIs now work
on browser `wasm32-unknown-unknown`, using `/v1/ws` unchanged. JSON remains the
outgoing default; binary MessagePack and mixed incoming frame codecs work too.
Limits, initial baselines, whole-batch publication, lag, replacement code/reason,
and local versus unknown write outcomes retain their SDK meanings.

Browser WebSockets support `ws://` and browser-managed `wss://`; an HTTPS page
normally needs a TLS proxy and `wss://`. TLS deployment was not tested here.
Browser control frames, certificates and networking restrictions are outside
Rust's socket API, and handshake errors may omit details. Browser receive size
checks occur after complete-message allocation; timers depend on event-loop
scheduling. Native TLS, JavaScript bindings and automatic reconnect remain
outside this increment. See [SDK usage](../tanuki-client/README.md#browser-webassembly).

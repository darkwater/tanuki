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
| `GET /v1/snapshot?select=/battery/*` | one selector | Selection-filtered retained snapshot |
| `GET /v1/sse?select=/battery/*` | one selector | Snapshot followed by live atomic update events |
| `PUT /v1/schemas/{name}` | complete schema declaration | Atomically install or replace one named schema |
| `PUT /v1/links/{name}` | `{"mount":"/view","target":"/canonical"}` | Atomically install or replace a writable subtree link |
| `DELETE /v1/links/{name}` | none | Remove a link and retract its projected nodes |

Batch operations use an `op` discriminator: `publish_state`, `publish_event`,
`define_input`, `claim_input`, `submit_desired`, `submit_command`,
`clear_desired`, and `remove_node`. Retained writes accept an expiry object:
`{"mode":"preserve"}`, `{"mode":"clear"}`, or
`{"mode":"set","duration":"PT1H"}`. Omission provisionally means
`preserve`: keep the existing absolute deadline, or use no deadline for a new
value. Claim releases similarly use `immediate` or `after` with a nonnegative
fixed ISO-8601 duration.

Successes are `{"ok":true,"data":...}`. Every adapter, extractor, routing,
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
arrays, and objects map directly to runtime values. The semantic forms are:

```json
{"$bytes":"AAEC/w=="}
{"$timestamp":"2023-11-14T22:13:20Z"}
{"$duration":"PT1H2M3S"}
{"$int":"9223372036854775807"}
{"$map":{"$bytes":"literal, not a tag"}}
```

Integers outside JavaScript's exact range encode with `$int`; unsigned values
outside signed 64-bit range are rejected. A literal map containing any reserved
tag key must use `$map`. Encoding is recursive and lossless for the accepted
initial value profile.

State and desired nodes use an explicit `current` wrapper. Desired
`"current":null` means no payload exists; `"current":{"value":null,...}`
means `Value::Null` was actually submitted. This distinction is preserved in
snapshots and updates.

## Subscription messages

The accepted stream uses a correlated snapshot as successful subscription
acknowledgement:

```json
{"type":"snapshot","request_id":"s1","sequence":42,"nodes":{},"warnings":[]}
{"type":"update","sequence":43,"changes":[]}
{"type":"reply","request_id":"w1","result":{}}
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
updates. The snapshot's `warnings` include managed-session replacement. Later
client messages are `write` messages whose `operations` array is identical to
the HTTP batch operation array:

```json
{"type":"write","request_id":"w1","operations":[{"op":"publish_state","topic":"/battery/laptop","value":87,"expiry":{"mode":"clear"}}]}
```

The write reply is sent before that same connection's resulting selected
update. Updates from unrelated concurrent writers can already be queued, so
clients distinguish correlated replies by `request_id` rather than assuming
every adjacent message is a pair. Decode failures whose ID cannot be recovered
use `request_id: null` and the malformed frame's codec.

Opening another managed WebSocket with the same client replaces the old
session. The old socket closes with code 4001 and reason `session_replaced`;
same-name stateless HTTP traffic does not replace it. Normal disconnect runs
claim cleanup. A slow core subscription closes with code 1013 and reason
`slow_consumer`; reconnecting starts from a fresh snapshot.

Inbound frames and individually encoded outbound messages are limited to 1
MiB. Each subscription currently buffers at most 64 complete commit batches;
the open byte-budget decision is tracked in `open-questions.md`.

MessagePack uses native primitives and binary values. Timestamps use the
standard MessagePack timestamp extension type `-1` and its 32-, 64-, or 96-bit
payload as appropriate. Tanuki application extension type `2` contains a UTF-8
ISO-8601 fixed duration.

Reference: the [MessagePack timestamp specification](https://github.com/msgpack/msgpack/blob/master/spec.md#timestamp-extension-type).

## Persistence format 3

The local snapshot is MessagePack with `format: "tanuki-snapshot"` and
`version: 3`. It is an internal restart format, not a client transport or an
acknowledgement log. It contains one coherent commit sequence and retained
node/definition metadata, installed schemas and link definitions, but no live sessions, claims,
event occurrences, or command occurrences. Future incompatible formats must use a new version;
unknown versions are treated as invalid snapshots. At startup invalid data is
copied beside the configured file with a `.bak` suffix (or `.bak.N` without
overwriting an earlier backup), a warning is logged, and Tanuki starts with
empty state. Ordinary read or backup I/O failures still fail startup.

## Native Rust SDK usage

`tanuki-protocol` supplies bidirectional v1 requests/messages and lossless value
codecs. The server supplies conversion functions `snapshot_message` and
`update_message` for authoritative core data. Payload-bearing tagged enum
decoding keeps the original JSON/MessagePack decoder through nested fields;
bytes, extension values and literal reserved-key maps now round-trip through
whole requests, snapshots and updates, not just standalone value fixtures.

`tanuki-client::Connection` exposes explicit hello/send/receive. `Session` adds
background reading, request correlation and atomic current-baseline listener
registration. Typed state/event/desired/command handles and atomic builders all
use that same write protocol; server validation remains unchanged. Exact-path
observers publish complete latest retained snapshots, while raw listeners expose
every admitted batch and explicit lag. Receipt sequences do not establish that
an observer caught up. Initial selections are immutable.

The supported native profile is `ws://`, JSON outgoing by default, optional
MessagePack frame encoding, 1 MiB messages, 64 pending requests, 64 raw batches
per listener and configurable 10-second connect/hello/write deadlines. There is
no idle timeout. TLS, browser bindings, Iced integration, automatic reconnect
and cross-topic Serde shapes are deferred. Full APIs, lifecycle semantics and
compiled examples are documented in [the client README](../tanuki-client/README.md).

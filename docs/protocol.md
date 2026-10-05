# Protocol

Protocol version 1 currently exposes JSON over HTTP. The representation remains
provisional until checkpoint B reviews snapshot/update/error DTOs; incompatible
changes are allowed before that checkpoint.

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

The current core rejects duplicate operations in the same input lifecycle slot,
same-topic output/removal combinations, and writes to the reserved `$`
namespace. The repeated-target details remain provisional until the external
batch contract is reviewed. WebSocket message envelopes, request correlation,
MessagePack extensions, and persistence format are not published yet.

## HTTP version 1

Writes require a validated UTF-8 `tanuki-client` header. This is attribution,
not authentication, and creates no managed session. Reads are anonymous.

| Method and path | Body/query | Meaning |
| --- | --- | --- |
| `POST /v1/state/{topic...}` | `{"value":72,"expiry":{"mode":"clear"}}` | Convenience retained-state write |
| `POST /v1/write` | `{"operations":[...]}` | One atomic operation batch |
| `GET /v1/snapshot?select=/battery/*` | one selector | Selection-filtered retained snapshot |

Batch operations use an `op` discriminator: `publish_state`, `publish_event`,
`define_input`, `claim_input`, `submit_desired`, `submit_command`,
`clear_desired`, and `remove_node`. Retained writes require an explicit expiry
object: `{"mode":"preserve"}`, `{"mode":"clear"}`, or
`{"mode":"set","duration":"PT1H"}`. This avoids choosing the still-open
omitted-expiry policy accidentally. Claim releases similarly use `immediate`
or `after` with a nonnegative fixed ISO-8601 duration.

Successes are `{"ok":true,"data":...}`. Every adapter, extractor, routing,
and core failure uses `{"ok":false,"error":{"code":"...","message":"..."}}`
with an appropriate HTTP status. A successful write returns its commit sequence
and structured warning list; it does not promise command execution or durable
storage.

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

## Provisional subscription messages

Checkpoint B remains open in `open-questions.md`. The implemented review shape
uses a correlated snapshot as successful subscription acknowledgement:

```json
{"type":"snapshot","request_id":"s1","sequence":42,"nodes":{}}
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

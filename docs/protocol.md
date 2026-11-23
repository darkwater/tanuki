# Protocol

Tanuki version 1 speaks JSON over HTTP, server-sent events, and JSON text or
MessagePack binary messages over one WebSocket. This is the reference for
writing a client. The representation is still provisional (checkpoint B);
incompatible changes are allowed before it is accepted, and are recorded at
the end of this document.

## Concepts

- A **topic** is a path such as `/battery/laptop`: `/`-separated non-empty
  segments. `/$...` is the reserved, read-only system namespace.
- A **selector** matches topics: `*` is one segment, `**` any number of
  segments (including none), and `{a,b}` alternatives within a segment, as in
  `/battery/*`, `/lamp/**` or `/tv/{volume,mute}`.
- A **node** is what exists at a topic. There are four kinds:

  | Kind | Retained? | Written by | Meaning |
  | --- | --- | --- | --- |
  | `state` | yes | `publish_state` | an observed fact, such as a battery level |
  | `event` | no | `publish_event` | something that happened; only the last publisher is retained |
  | `desired` | yes | `submit_desired` | a requested value for an input, such as a target volume |
  | `command` | no | `submit_command` | a one-off request to an input |

  Desired and command nodes are **inputs**: they are defined first and can be
  **claimed** by one managed session, the input's controller.
- Every write is one atomic **batch** of operations. An accepted batch is one
  **commit** with a global **sequence** number and produces one **update**: the
  complete set of changes, in order. A rejected batch has no effect.
- A **client name** (such as `laptop` or `kitchen tablet`) attributes writes.
  It is attribution, not authentication.
- A **managed session** is a WebSocket connection that can claim inputs. A
  later managed connection with the same name replaces it. Stateless
  connections and HTTP requests are attributed but never claim, replace or get
  replaced.

Sequences are global and increase; a subscription skips commits that touched
nothing it selects, so a gap is not a loss. There is no replay: a client that
reconnects starts again from a fresh snapshot.

## Values

A value is null, a boolean, a signed 64-bit integer, a finite 64-bit float, a
string, bytes, a list, or a map with string keys. There are no timestamp or
duration values: a date or interval inside a payload is a string or integer,
and schemas can validate or cast it.

In JSON, ordinary values map directly. Three tagged forms cover the rest:

```json
{"$bytes":"AAEC/w=="}
{"$int":"9223372036854775807"}
{"$map":{"$bytes":"a literal key, not a tag"}}
```

Integers outside JavaScript's exact range (±(2⁵³−1)) use `$int`; unsigned
values beyond the signed 64-bit range are rejected. A map that has a key
`$bytes`, `$int` or `$map` must be wrapped in `$map`. Other `$` keys are
ordinary. Floats round-trip bit-exactly.

MessagePack uses native types, including `bin` for bytes, and no extension
types; a message containing one is rejected.

Each value's JSON encoding, measured after any schema or alias cast, is limited
to 256 KiB (`value_too_large`, HTTP 413). One value therefore always fits in a
1 MiB WebSocket message.

## Nodes

Snapshots map topics to nodes; updates carry nodes in their changes. A node is
an object tagged by `kind`:

```json
{"kind":"state","current":{"value":82,
  "last_write":{"client":"laptop","session":7,"at":"2026-11-22T10:00:00Z"},
  "expires_at":"2026-11-22T10:01:30Z"}}
{"kind":"event","last_publisher":{"client":"doorbell","session":null,"at":"2026-11-22T10:00:00Z"}}
{"kind":"desired","claim":{"id":3,"owner":"lamp","session":9,"release":{"mode":"immediate"}},
  "current":{"value":70,"last_write":{"client":"phone","session":null,"at":"..."},"expires_at":null}}
{"kind":"command","claim":null}
```

- `current` (state and desired) is the retained value with its provenance:
  who wrote it (`session` is null for stateless writes) and when, and the
  optional expiry. A desired node without a pending request has
  `"current":null`; `"current":{"value":null,...}` means `null` was submitted.
- `claim` (desired and command) names the controlling session and what happens
  to the claim when that session disconnects: `{"mode":"immediate"}`, or
  `{"mode":"after","duration":"PT30S"}` to keep it for a grace period.
- Times are RFC 3339 timestamps; durations are ISO 8601 fixed durations.

## Write operations

A batch is a list of operations tagged by `op`, applied in order:

| `op` | Fields | Effect |
| --- | --- | --- |
| `publish_state` | `topic`, `value`, `expiry`? | Retain a state value |
| `publish_event` | `topic`, `value` | Emit an event occurrence |
| `define_input` | `topic`, `kind` (`desired` or `command`) | Create an input |
| `claim_input` | `topic`, `release` | Claim an input (managed sessions only) |
| `submit_desired` | `topic`, `value`, `expiry`? | Request a desired value |
| `submit_command` | `topic`, `value` | Emit a command occurrence |
| `clear_desired` | `topic` | Clear the pending desired value |
| `remove_node` | `topic` | Remove a node |

`expiry` is `{"mode":"clear"}` (no deadline; the default when omitted),
`{"mode":"set","duration":"PT90S"}` (a lease, renewed by writing again), or
`{"mode":"preserve"}` (keep the existing absolute deadline of a node of the
same kind). A writer that omits expiry never inherits a deadline another writer
chose. `release` is `{"mode":"immediate"}` or `{"mode":"after","duration":"PT30S"}`.

A batch may define, claim and submit on one input in operation order. Writes to
`/$...` are rejected. Repeated operations on one topic run in order; the update
shows each retained node's final shape plus every event and command occurrence.
An accepted write returns a **commit**:

```json
{"sequence":43,"warnings":[]}
```

Warnings are successful-operation diagnostics, such as a schema warning or a
node changing kind; each has a `code` and context. A commit means the server
accepted the write, not that a device acted.

## HTTP

Successful responses are `{"ok":true,"data":...}`; failures are
`{"ok":false,"error":{"code":"...","message":"..."}}` with the status from the
error table below.

| Method and path | Body or query | Result |
| --- | --- | --- |
| `POST /v1/write` | `{"operations":[...]}` | commit |
| `POST /v1/state/{topic...}` | `{"value":72,"expiry":{...}}` (expiry optional) | commit |
| `GET /v1/snapshot?select=...` | one or more `select` | `{"sequence":42,"nodes":{...}}` |
| `GET /v1/sse?select=...` | one or more `select` | event stream (below) |
| `PUT /v1/schemas/{name}` | schema declaration | installation result |
| `PUT /v1/links/{name}` | `{"mount":"/view","target":"/canonical"}` | installation result |
| `DELETE /v1/links/{name}` | — | removal result |

Writes, schema and link changes need a client name in the `tanuki-client`
header or the `client` query parameter (they must agree if both are given).
Reads are anonymous. Repeat `select` for several selectors
(`?select=/battery/*&select=/lamp/**`); URL-encode braces.

```sh
curl -H 'tanuki-client: phone' -H 'content-type: application/json' \
  -d '{"value":72,"expiry":{"mode":"set","duration":"PT1H"}}' \
  http://127.0.0.1:5167/v1/state/battery/phone
```

A schema declaration is `{"force":false,"rules":[...]}`. Each rule has a
`selector`, `enforcement` (`warn` or `deny`), optional `nullable`, optional
`node_kind`, optional `expected_update_interval` (an ISO 8601 duration such as
`"PT5M"`), a tagged `validator` (`any`, `kind`, `integer_range`, `float_range`,
`string_enum`) and an optional cast (`string_to_integer`, `string_to_float`,
`string_to_bool`). Installation replaces the schema of that name atomically and
by default rejects existing denying violations; `force:true` removes them
instead. Patterns of different schemas may not overlap.

A link projects its target subtree beneath its mount in snapshots and updates;
writes to an alias are translated to the canonical topic (alias schema first,
then canonical schema). Deleting an absent link succeeds.

## Server-sent events

`GET /v1/sse?select=...` is an anonymous read-only subscription. The first
event is `snapshot`, then one `update` event per selected commit. `data` is the
JSON of the snapshot or update (without `type` or `request_id`), and `id` is
its sequence:

```text
event: snapshot
id: 42
data: {"sequence":42,"nodes":{}}

event: update
id: 43
data: {"sequence":43,"changes":[]}
```

A keep-alive comment follows 15 seconds without data. `Last-Event-ID` is
ignored: reconnecting yields a new snapshot. A subscriber that falls 64 commits
behind is disconnected.

## WebSocket

`GET /v1/ws` upgrades to a WebSocket. Every text frame is one JSON message and
every binary frame one MessagePack message; a client may mix them. A reply uses
the codec of the request's frame; unsolicited snapshots and updates use the
codec of the hello. Each message is at most 1 MiB and must contain exactly one
encoded message (trailing data rejects it before any effect).

### Client messages

Every client message has a `type` and a `request_id` (any string) that the
server echoes in its answer.

```json
{"type":"hello","request_id":"h","client":"laptop","selectors":["/battery/*"],"session":"managed"}
{"type":"write","request_id":"w1","operations":[{"op":"publish_state","topic":"/battery/laptop","value":82}]}
{"type":"select","request_id":"s1","selectors":["/battery/*","/lamp/**"]}
```

- `hello` must be the first message. `session` is `"managed"` (the default) or
  `"stateless"`. The selectors start the connection's subscription (possibly
  empty).
- `write` sends one batch with the same operations as HTTP.
- `select` replaces the subscription's selectors.

### Server messages

```json
{"type":"snapshot","request_id":"h","sequence":42,"nodes":{},"warnings":[]}
{"type":"update","sequence":43,"changes":[...]}
{"type":"reply","request_id":"w1","result":{"sequence":43,"warnings":[]}}
{"type":"error","request_id":"w1","error":{"code":"empty_batch","message":"..."}}
```

- `snapshot` answers `hello` and `select`: every selected node at one commit.
  A hello's snapshot `warnings` report, for example, that it replaced an older
  managed session.
- `update` is one complete commit, filtered to the selection. Changes are
  tagged `type`: `upsert` (`topic`, complete `node`), `removed` (`topic`,
  `previous` node), `event` and `command` (`topic`, plus `event` or `command`
  holding `value` and `provenance`). Apply all changes of an update together.
- `reply` answers a successful `write` with its commit.
- `error` answers a failed request. A message that could not be decoded at all
  gets `request_id: null`.

### Ordering guarantees

- A hello is answered by exactly one snapshot before any other message.
- A `select` is answered by a snapshot of the new selection; updates queued
  for the old selection are dropped, because the snapshot already includes
  them.
- A write's own update (if selected), and every update queued before it, is
  sent before its reply. A client that applies updates in order therefore
  already reflects its write when the reply arrives. Updates from other writers
  can arrive at any time, so match replies by `request_id`.

### A sample conversation

```text
→ {"type":"hello","request_id":"h","client":"lamp","selectors":["/lamp/**"]}
← {"type":"snapshot","request_id":"h","sequence":10,"nodes":{},"warnings":[]}
→ {"type":"write","request_id":"w1","operations":[
    {"op":"define_input","topic":"/lamp/desired","kind":"desired"},
    {"op":"claim_input","topic":"/lamp/desired","release":{"mode":"immediate"}}]}
← {"type":"update","sequence":11,"changes":[{"type":"upsert","topic":"/lamp/desired",
    "node":{"kind":"desired","claim":{"id":1,"owner":"lamp","session":4,"release":{"mode":"immediate"}},"current":null}}]}
← {"type":"reply","request_id":"w1","result":{"sequence":11,"warnings":[]}}
   (another client submits 70 over HTTP)
← {"type":"update","sequence":12,"changes":[{"type":"upsert","topic":"/lamp/desired",
    "node":{"kind":"desired","claim":{...},"current":{"value":70,"last_write":{...},"expires_at":null}}}]}
→ {"type":"write","request_id":"w2","operations":[{"op":"publish_state","topic":"/lamp/brightness","value":70}]}
← {"type":"update","sequence":13,"changes":[{"type":"upsert","topic":"/lamp/brightness","node":{...}}]}
← {"type":"reply","request_id":"w2","result":{"sequence":13,"warnings":[]}}
```

### Sessions, claims and disconnects

A managed hello opens a session; a later managed hello with the same client
name replaces it, and the old connection closes with 4001. When a managed
session ends (close, replacement, timeout), its claims are released, or kept
for their grace period. Stateless connections cannot claim
(`session_required`) and are never replaced. After reconnecting, a controller
must claim its inputs again.

### Close codes and liveness

A close frame carries a numeric code and a stable reason string:

| Code | Reason | When |
| --- | --- | --- |
| 1001 | `server_shutdown` | the server is shutting down |
| 1008 | `hello_timeout` | no hello within 10 seconds |
| 1009 | `message_too_big` | an outgoing message, such as a snapshot of many large values, would exceed 1 MiB |
| 1011 | `peer_timeout` | nothing (not even a pong) received for 60 seconds |
| 1013 | `slow_consumer` | the connection fell 64 commits behind |
| 4001 | `session_replaced` | a newer managed connection with the same name |

The server pings every 20 seconds; WebSocket libraries answer automatically
while the client keeps reading. A send that does not complete within 10 seconds
drops the connection. The server acknowledges a client's close frame. After any
close, reconnect with a new hello and replace cached state with the new
snapshot; retrying after 4001 would take the name back from the newer session.

## Diagnostics

Active conditions appear as read-only state nodes under `/$diagnostics/**`:

- `/$diagnostics/freshness/<topic...>`: a value whose schema's
  `expected_update_interval` elapsed. Its value is a map with `code`
  (`overdue_update`), `topic`, `expected_update_interval` and `last_updated`
  (strings).
- `/$diagnostics/links/<name>`: a link disabled by a schema.

Onset is an upsert and recovery a removal. Select them like any topic
(`/**` includes them).

## Error codes

HTTP error bodies and WebSocket `error` messages share one set of codes. A
client should treat unknown codes as generic failures (the Rust type decodes
them as `ErrorCode::Unknown`).

| Code | HTTP | Meaning |
| --- | --- | --- |
| `hello_required` | — | The first WebSocket message was not `hello` |
| `already_initialized` | — | A second `hello` |
| `invalid_message` | — | A WebSocket message could not be decoded |
| `invalid_json`, `invalid_query` | 400 | Malformed HTTP body or query (including missing or invalid `select`) |
| `missing_client`, `invalid_client`, `conflicting_client` | 400 | HTTP attribution absent, invalid or contradictory |
| `invalid_topic`, `invalid_duration`, `negative_duration`, `empty_batch` | 400 | Invalid request fields |
| `deadline_out_of_range` | 400 | Expiry or grace beyond the timestamp range |
| `session_required` | 400 | Claiming needs a managed session |
| `invalid_schema`, `invalid_link` | 400/409 | Invalid schema or link declaration or topology |
| `system_topic` | 403 | Writes to `/$...` are forbidden |
| `input_not_defined` | 404 | Submit, claim or clear on an undefined input |
| `not_found`, `method_not_allowed` | 404/405 | Unknown endpoint or method |
| `session_expired` | 409 | The managed session was replaced or disconnected |
| `claim_authority_required` | 409 | Another session's claim controls the input |
| `wrong_node_kind` | 409 | The operation does not fit the existing node's kind |
| `schema_overlap`, `existing_schema_violations` | 409 | Schema installation conflicts |
| `value_too_large` | 413 | A value's JSON encoding exceeds 256 KiB |
| `schema_violation` | 422 | A denying schema rejected the batch |
| `capacity_exhausted`, `internal_error` | 500 | Server-side failure |
| `server_shutdown` | 503 | The server is shutting down |

## Rust crates

`tanuki-protocol` has the message types (`ClientMessage`, `ServerMessage`,
`WireOperation`, `NodeView`, `ChangeView`, `CommitView`, `ErrorCode`, ...) with
validated `TopicPath`, `Selector` and `ClientName`, and `Value`. `Value` is an
ordinary Serde type with the encoding above (tagged in JSON, native in
MessagePack); `to_value`/`from_value` and the views' `decode::<T>()` convert any
Serde type, where `Vec<u8>` becomes bytes and fixed arrays such as `[u8; 3]`
stay lists. `SelectedView` is a minimal cache that applies snapshots and updates.

`tanuki-client-core` implements these messages for Rust, native and in the
browser: a raw `Connection`, and a reconnecting `Client` that follows the
reconnect rules above (new hello, replace state from the snapshot, no retry
after 4001, re-claim through its setup batch). See
[its README](../tanuki-client-core/README.md).

## Persistence

The server's restart snapshot is MessagePack with `format: "tanuki-snapshot"`,
`version: 3`: one coherent commit with retained nodes, input definitions,
schemas and links, but no sessions, claims or occurrences. It is internal, not
an API. Invalid data is moved aside to `.bak` (or `.bak.N`) with a warning, and
the server starts empty. A version 3 snapshot that still contains a timestamp or
duration value, or a schema requiring one, is invalid since those values were
removed.

## Changes

- 2026-11-22: `Value` is the wire value type in Rust (`WireValue` removed);
  replies carry a typed commit (`CommitView`; JSON unchanged); input nodes lost
  the empty `"definition": {}`; HTTP reads accept repeated `select`; the server
  acknowledges client close frames.
- 2026-11-21: timestamp and duration values removed (`$timestamp`,
  `$duration`, MessagePack extension types); omitted expiry means `clear`;
  stateless sessions and `select`; updates before replies; shared error codes;
  liveness timeouts and close codes; 256 KiB value limit.

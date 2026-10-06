# Tanuki

Tanuki is an early personal automation fabric for named state, events, desired
inputs, and commands. One transport-independent core supplies atomic writes,
managed input ownership, coherent subscriptions, expiry, schemas, writable
linked views, active diagnostics, and best-effort restart persistence to HTTP
snapshot/SSE and WebSocket clients.

Tasks 00–12 of the implementation plan are complete for the initial in-repo
release. Real-device deployment is not implied; remaining product questions and
provisional limits are listed in
[docs/open-questions.md](docs/open-questions.md).

## Workspace layout

The repository root is a virtual Cargo workspace. All packages live directly
at the root:

- `tanuki/` — authoritative server library, binary and integration tests
- `tanuki-protocol/` — shared wire types/codecs and validated primitives
- `tanuki-client/` — native Rust SDK, examples and client tests

Shared documentation, `Cargo.lock`, toolchain configuration and CI remain at the
workspace root. See [the SDK guide](tanuki-client/README.md) for typed publication,
atomic writes and raw/latest-state observations.

## Run it

The repository pins `nightly-2026-10-01`, including rustfmt and Clippy.

```sh
cargo run -p tanuki
```

The default listener is `127.0.0.1:5167`. Set `TANUKI_LISTEN` to another socket
address. Retained state is saved every 30 seconds and at orderly shutdown to
`tanuki.db`; set `TANUKI_SNAPSHOT` to choose another path. Invalid snapshot
data is backed up beside that file and startup continues empty. Client
names are attribution and session identity, not authentication, so remote
exposure must be an explicit deployment decision.

Publish and read a phone battery through stateless HTTP:

```sh
curl -sS \
  -H 'content-type: application/json' \
  -H 'tanuki-client: phone task' \
  --data '{"value":72,"expiry":{"mode":"set","duration":"PT1H"}}' \
  http://127.0.0.1:5167/v1/state/battery/phone

curl -sS \
  'http://127.0.0.1:5167/v1/snapshot?select=/battery/*'

curl -N \
  'http://127.0.0.1:5167/v1/sse?select=/battery/*'
```

The SSE stream starts with a complete `snapshot` event and then sends atomic
`update` events. Reconnect always starts from a fresh snapshot; event IDs are
informational commit sequences, not replay cursors.

`POST /v1/write` accepts a nonempty atomic `operations` array. Supported
operations are `publish_state`, `publish_event`, `define_input`, `claim_input`,
`submit_desired`, `submit_command`, `clear_desired`, and `remove_node`.
Stateless HTTP can define or submit inputs but cannot claim them.

## Schemas

Install or replace a named schema atomically with
`PUT /v1/schemas/{name}`. This example constrains battery state to integer
percentages and explicitly casts strings such as `"72"` before revalidation:

```sh
curl -sS -X PUT \
  -H 'content-type: application/json' \
  -H 'tanuki-client: administrator' \
  --data '{
    "rules": [{
      "selector": "/battery/*",
      "enforcement": "deny",
      "node_kind": "state",
      "validator": {"type": "integer_range", "minimum": 0, "maximum": 100},
      "cast": "string_to_integer",
      "expected_update_interval": "PT5M"
    }]
  }' \
  http://127.0.0.1:5167/v1/schemas/battery
```

Normal installation reports existing denying violations and changes nothing.
Add `"force":true` to remove invalid state values or clear an invalid desired
current value while preserving its definition and claim. Warning rules accept
the operation and return diagnostics. Installed schemas survive snapshots and
restart. See [docs/protocol.md](docs/protocol.md) for the complete initial
validator and cast vocabulary.

## Writable links and diagnostics

Install a writable subtree view with an attributed request:

```sh
curl -sS -X PUT \
  -H 'content-type: application/json' \
  -H 'tanuki-client: administrator' \
  --data '{"mount":"/dashboard/battery","target":"/battery"}' \
  http://127.0.0.1:5167/v1/links/dashboard-battery
```

Readers and writers can then use `/dashboard/battery/...`; canonical storage
remains under `/battery/...`. Alias schemas run before canonical schemas. A
canonical value rejected by the alias view commits canonically but disables the
whole link until data or policy repairs it. `DELETE /v1/links/{name}` removes a
definition. Links and schemas survive restart.

Select `/$diagnostics/**` to observe retained active conditions. Overdue values
appear below `/$diagnostics/freshness/...`; disabled links appear below
`/$diagnostics/links/...`. Recovery removes the condition. These paths are
read-only and are never governed by user schemas.

## WebSocket

Connect to `/v1/ws`. The first frame is a hello and determines the codec for
unsolicited snapshots and updates. Every text frame contains JSON and every
binary frame contains MessagePack; request frames may mix codecs, and each
correlated reply uses its request's codec.

```json
{"type":"hello","request_id":"h1","client":"laptop","selectors":["/battery/*","/lamp/*"]}
```

The first server message is the correlated complete snapshot. Subsequent
messages are atomic `update` batches and request-correlated `reply` or `error`
messages. A write uses the same operation objects as HTTP:

```json
{"type":"write","request_id":"w1","operations":[{"op":"publish_state","topic":"/battery/laptop","value":87,"expiry":{"mode":"clear"}}]}
```

A second managed WebSocket with the same client name replaces the old session.
Same-name HTTP traffic remains stateless and cannot do that. Reconnect after a
slow-client closure obtains a fresh snapshot; there is no replay log.

See [docs/protocol.md](docs/protocol.md) for current wire details and semantic
JSON/MessagePack value mappings.

## Quality gates

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --release
```

The suites include real loopback HTTP/WebSocket clients, a simulated room with
downstream actors, linked-view invalidation/recovery, controlled-time
expiry/freshness/grace checks, atomic-save failure coverage, orderly restart,
and a production-binary restore smoke test.

## Documentation map

- [AGENTS.md](AGENTS.md) — engineering requirements and working conventions
- [Specification](docs/spec.md) — accepted product behavior
- [Architecture](docs/architecture.md) — implemented modules and data flow
- [Protocol](docs/protocol.md) — current HTTP, WebSocket, codec, and snapshot formats
- [Runtime procedures](docs/procedures.md) — mutation, sessions, timers, persistence, shutdown
- [Testing](docs/testing.md) — acceptance-scenario evidence
- [Implementation plan](docs/implementation-plan.md) — sequential task cards
- [Decisions to review](docs/decisions-to-review.md) — accepted and proposed choices
- [Open questions](docs/open-questions.md) — prioritized user-review ledger
- [Living user stories](docs/user-stories/USER-STORIES.md) — intended versus tested/deployed use

Historical discussion lives under `docs/history/` for rationale only; current
documents take precedence when they disagree.

## Current limitations

TCP/MQTT adapters, authentication, a Unix-socket listener, large-blob
transfer, event replay, and outgoing queued-byte accounting are not yet
implemented. Link chains are deliberately unsupported.
Persistence is periodic best effort rather than a WAL: an
acknowledged write can be lost if the process crashes before the next completed
snapshot. Wire shapes are versioned and remain pre-1.0.

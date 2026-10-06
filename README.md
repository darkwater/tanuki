# Tanuki

Tanuki is an early personal automation fabric for named state, events, desired
inputs, and commands. One transport-independent core supplies atomic writes,
managed input ownership, coherent subscriptions, expiry, and best-effort
restart persistence to HTTP and WebSocket clients.

Tasks 00–08 of the implementation plan are implemented. Schema enforcement and
linked views are the next architecture checkpoint; the current prioritized
decisions are in [docs/open-questions.md](docs/open-questions.md).

## Run it

The repository pins `nightly-2026-10-01`, including rustfmt and Clippy.

```sh
cargo run
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
```

`POST /v1/write` accepts a nonempty atomic `operations` array. Supported
operations are `publish_state`, `publish_event`, `define_input`, `claim_input`,
`submit_desired`, `submit_command`, `clear_desired`, and `remove_node`.
Stateless HTTP can define or submit inputs but cannot claim them.

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
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo test --release --all-targets --all-features
```

The suites include real loopback HTTP/WebSocket clients, a simulated room with
downstream actors, controlled-time expiry/grace checks, atomic-save failure
coverage, orderly restart, and a production-binary restore smoke test.

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

Schemas, linked views, freshness diagnostics, TCP/MQTT/SSE, authentication,
large-blob transfer, event replay, and a polished client SDK are not yet
implemented. Persistence is periodic best effort rather than a WAL: an
acknowledged write can be lost if the process crashes before the next completed
snapshot. Wire shapes remain provisional pending checkpoint-B review.

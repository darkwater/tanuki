# Tanuki system inspector — implementation handoff

2026-10-31. Task definitions and proposed defaults; implementation evidence is
recorded separately below.
User goal: a UI similar to mqttui that eventually shows the entire public
server state, including node kinds and applicable schemas. See
[US-13](user-stories/STORY-13-system-inspector.md).

## Progress — 2026-10-31

- Checkpoint A confirmed by the user: native read-only terminal application,
  public ordinary topics and exposed reserved branches, incremental delivery,
  explicit reconnect and the other checkpoint defaults below.
- Task 01 implemented and headlessly tested in `tanuki-ui`: one SelectedView,
  path-based selection/tree/filtering and safe, untruncated detail formatting.
  See [testing.md](testing.md#inspector-task-01--pure-model-and-formatting).
- Provisional local defaults: initially collapsed branches; case-sensitive full
  path substring search temporarily expands matching ancestor context; selection
  prefers a visible ancestor, then nearest surviving old row (preceding on ties),
  then first new row. These remain the navigation defaults for Task 02.
- Task 02 implemented: one SDK session/raw listener, registration baseline,
  complete batches, stale disconnect/error/lag, explicit fresh-baseline reconnect
  and joined close. Real production-server loopback tests verify wildcard future
  topics, atomic updates, metadata/expiry, diagnostics and duplicate replacement.
- Task 03 implemented: Ratatui 0.30.2/Crossterm 0.29.0 terminal, controls and
  CLI, safe Unicode-aware clipping/scrolling, stale status, help and resize
  fallback. An owning guard restores terminal state on exit/errors/unwinding.
  Single cancellable connection job keeps input responsive without update queues.
- Milestone 1 is implemented and verified with headless backend tests, production
  loopback tests and separate actual-binary POSIX PTY smoke checks. This is not
  complete policy/public-state coverage or verified deployment.
- Next: Task 04 only, checkpoint B public inspection design review. Do not expand
  server APIs or add schema/link UI until that checkpoint is agreed.
- Verified scope gap: `/**` subscriptions include exposed `/$diagnostics/**`.
  The server does not currently project `/$connections/**`. The checkpoint's
  expectation that the observer itself appears as a system node is therefore
  not implemented by the server; Task 02 does not invent such nodes or change
  server behavior. Complete-state coverage remains unmet.

## Instructions to the implementing model

Implement one numbered task at a time, in order. Read `AGENTS.md` first and
follow its TDD, documentation, quality-gate, commit and push requirements.
Do not implement all future features while completing one task. Report the
finished task, evidence, limitations and the next task. Do not spawn agents
unless the user explicitly asks for them.

Read these before changing behaviour:

- `docs/spec.md`, `docs/design.md`, `docs/test-plan.md`;
- `docs/architecture.md`, `docs/protocol.md`, `docs/procedures.md`;
- `docs/user-stories/STORY-13-system-inspector.md` and US-09/US-11;
- `tanuki-client/README.md` and `docs/toolchain.md`.

This document proposes defaults to reduce implementation decisions. They are
not previously accepted product requirements. Present checkpoint A once before
implementation; use subsequent user instructions as authority. Do not repeatedly
seek permission for routine helpers or already authorized tasks.

## Checkpoint A — proposed scope

Propose a native Rust terminal application named `tanuki-ui`, inspired by the
[mqttui terminal client](https://github.com/EdJoPaTo/mqttui). Use a live topic tree
on the left, details on the right, and connection/help status at the bottom.

Proposed defaults:

- Read-only inspection. No publish, edit, claim, schema management or deletion UI.
- Public means state deliberately exposed by the client protocol, including
  ordinary topics and exposed reserved system branches. No process-memory dump,
  authentication system or permissions redesign.
- First milestone uses existing APIs and explicitly labels schema information
  unavailable. Second milestone adds public inspection APIs and policy details.
- Separate root-level workspace package; use the native SDK for the live view.
- Default URL `ws://127.0.0.1:5167/v1/ws`; configurable URL and client name.
  Generate a process-distinct default name so two local inspectors do not
  displace each other; retain normal server replacement semantics for explicit
  duplicate names. This observer connection will itself appear in system state.
- Explicit reconnect key, fresh snapshot on reconnect, no automatic retries or
  replay. Preserve old display as visibly disconnected/stale until replaced.
- Evaluate Ratatui with Crossterm for terminal rendering/input. Check official
  current APIs, select compatible versions against the pinned compiler, and
  commit the lockfile. Do not upgrade nightly just to use a UI dependency.

Tradeoff: a terminal application reuses the existing Rust SDK and works over SSH,
but does not provide a browser or Iced interface. Confirm this interpretation
of mqttui and the public surface before building the UI.

## Existing code and constraints

- Root `Cargo.toml`: virtual workspace; add the UI as another root-level member.
- `tanuki-client/src/session.rs`: `Session::connect`, `listen_updates`,
  `RawListener::initial_snapshot`, `recv`, and joined `Session::close`.
- `tanuki-protocol/src/view.rs`: `SelectedView::from_snapshot`, `apply`, `nodes`,
  `sequence`. Reuse whole-batch application and its occurrence separation.
- `tanuki-protocol/src/wire.rs`: four `NodeView` variants, current value,
  provenance, claim, snapshot/update and occurrence DTOs.
- `tanuki-client/src/observe.rs`: existing observers select exact paths. They
  do not discover arbitrary future paths; do not build a wildcard inspector by
  freezing today's list of paths into `observe_batch`.
- `tanuki/src/transport.rs`: common response/error envelopes, request extractors,
  HTTP routes and WebSocket adapter.
- `tanuki/src/runtime.rs`: server lifecycle and access to authoritative state.
- `tanuki/src/core/`: authoritative mutations, reads, subscriptions and policy.
- `tanuki/src/schema.rs` and `tanuki/src/link.rs`: policy registries and matching;
  verify their actual interfaces before editing.

Schemas currently have installation but no public enumeration endpoint; links
have installation/removal but no public enumeration endpoint. Node snapshots
alone cannot provide policy introspection. Do not read the server's snapshot
file or depend on server internals from the UI to bypass this gap.

Broad ordinary subscriptions and explicitly selected system paths must follow
existing selector semantics. Verify what `/**` actually includes using a real
server test; do not copy the schema matcher's ordinary-only restriction into
subscription handling. No fake node should be invented for a directory prefix.

## Task 01 — pure inspector model and formatting

Deliver a small library plus thin binary in `tanuki-ui/`. Suggested modules:
`model`, `format`, `app`, `terminal`; add a network module only if it simplifies
ownership. Production dependencies flow from UI to client/protocol, never server.
The server may be a development dependency for integration tests.

Model owns one `SelectedView`, selected full path, expanded prefixes, filter and
connection status. A prefix can both contain a real node and have descendants.
Keep node identity by full path, not visible row index. Build tree rows from
current nodes and prefixes; preserve expansion/selection through updates where
possible. Choose the nearest surviving visible row when selection disappears.

Tests first:

1. Empty snapshot renders an explicit empty state.
2. `/lamp` and `/lamp/hue` remain separately inspectable.
3. All four kinds have distinct labels; payload value type is a separate label.
4. Missing desired payload, explicit Null, absent node and instant node differ.
5. Atomic paired changes are visible together; removal clears stale details.
6. Search retains ancestor context; collapse and removal do not corrupt selection.
7. Metadata-only updates refresh details; legal sequence gaps remain accepted.
8. Bytes and semantic time values have readable representations. Long values
   are truncated in rows but inspectable via scrolling in details.

Treat payloads, topic names and client names as untrusted terminal text. Display
control characters safely; do not pass embedded escape sequences through to the
terminal. Preserve the underlying value when producing display text.

Done: model/format tests pass without a terminal or running server. No network
or server changes are necessary for this task.

## Task 02 — live view ownership and connection lifecycle

Connect through `Session`, then obtain a raw listener. Initialize from that
listener's registration baseline, not the earlier session hello snapshot, and
apply every subsequent complete batch with `SelectedView::apply`.

A single async application loop can select between network batches, input and
render ticks. Rendering may skip frames; applying network input may not skip
batches. Avoid new unbounded queues. Keep terminal drawing short and synchronous;
never hold a server lock or await a network operation while drawing.

Explicit connection states: connecting, live, disconnected, and lagged/error.
On abnormal termination stop claiming the cached view is current. Reconnect
replaces the entire baseline and session; it does not append old occurrences.
On exit close/join the session. Do not add reconnect behaviour to the SDK merely
for this application.

Tests with production server logic on loopback:

- Initial populated and empty snapshots; a later wildcard-matching topic appears.
- One atomic batch updates paired nodes; metadata-only changes and expiry appear.
- Reserved system information is included according to the agreed scope.
- Disconnect/duplicate-name replacement becomes visible and permits reconnect.
- Reconnect removes stale cached nodes absent from the new baseline.

Use existing SDK tests as evidence for driver guarantees; add focused tests for
new UI state transitions. Deterministically inject listener termination to test
lag handling; do not create sleep-based timing tests.

Done: application state follows a real server without polling or manual refresh.

## Task 03 — usable terminal inspector, milestone 1

Add tree/detail/status layout. Suggested keys: arrows or j/k to navigate,
left/right to collapse/expand, `/` to filter, Escape to clear filter, Tab to
switch pane, PageUp/PageDown to scroll details, `r` reconnect, `?` help, `q`
and Ctrl-C exit. Show key hints. Small terminals must show a useful fallback,
not panic or calculate negative dimensions.

Details show full path, node kind, value type/value, provenance, expiry and
claim metadata where supplied. Show schema details as “not available from this
server API”; do not show “no schema”. System nodes are visually identifiable.
No occurrence history is required for this milestone: event/command nodes show
metadata and clearly say they have no retained payload. Do not silently present
the last observed occurrence as retained state.

Use an owning terminal guard to restore raw mode, alternate screen and cursor
on normal exit, errors and unwinding. Handle resize and Ctrl-C. Avoid broad
panic catching or a custom terminal framework. Verify restoration manually in
an interactive terminal and test normal/error cleanup boundaries where feasible.

Test layout with the chosen library's test backend: empty/populated views,
selection, long/unicode/control-character values, tiny dimensions, stale/error
banner and help. Tests should assert meaningful visible content, not only huge
screenshots of the current implementation.

Update README with run command and controls; record US-13.A1/A2 evidence and
remaining A3/A4 gaps. Run workspace gates below. Commit/push the milestone.
This is a useful deliverable; do not call the complete inspector finished.

## Task 04 — checkpoint B: coherent public introspection design

Prepare concrete DTOs, core/runtime API and wire examples before implementation.
Review with the user because this expands the public server contract.

Recommended bounded starting point: one read-only HTTP inspection endpoint
returning a single captured view of node data, installed schema definitions,
link definitions/status, and explanations for a requested node. Capture owned
data at one authoritative core boundary, release the lock, then serialize.
Reuse existing common errors and recurring request extractors. Keep external
DTOs in `tanuki-protocol`; do not serialize private registries directly.

Specify endpoint/query names, response limits, deterministic ordering and
behaviour for absent topics, aliases, disabled aliases and reserved paths.
An explanation lists matching rules and their stage (alias or canonical), with
schema identity, selectors, enforcement, validators, casts and freshness policy.
It describes policy; it must not perform a write, cast a value, refresh metadata,
log a validation warning or alter link status. Reuse authoritative matching rules.

Important consistency constraint: current commit sequences need not advance for
configuration changes with no visible node delta. Never use that sequence alone
as proof that displayed schema configuration is current. For the first version,
prefer explicitly refreshed, coherent inspection snapshots labeled “captured at
refresh”, separate from the live node pane. A selected-node inspection response
must carry its own node data from that same capture. Do not merge independently
fetched policy into live nodes and imply atomic consistency.

Tradeoff: explicit refresh is simpler and honest, but policy is not live between
refreshes. If live policy is required immediately, design a configuration revision
and notification contract here, including changes matching no nodes. Do not
invent one ad hoc inside a UI handler. Keep global node sequence semantics intact.
Record the chosen behaviour in spec/protocol/procedures and the US-13 criteria;
never weaken the eventual complete-state goal to match an incremental snapshot.

## Task 05 — implement introspection after checkpoint B

Write core and real HTTP tests before implementation:

- Enumerate all installed schemas, including ones matching no nodes.
- Explain overlapping rules inside a schema and explicit no-match results.
- System nodes correctly have no user-schema enforcement.
- Explain alias and canonical stages without pretending an alias cast mutates
  canonical state during inspection.
- Enumerate active/disabled links and their targets/status, including views with
  no currently visible nodes.
- Replacement/removal and schema changes without node deltas appear on refresh.
- Returned node and policy data come from the same capture under concurrent work.
- Invalid paths/queries use the common error envelope.
- Inspection leaves nodes, metadata, sequence and diagnostics unchanged.

Add only the public read surface approved at checkpoint B. Do not refactor
validation wholesale. Keep server correctness independent of the inspector.
Document size limits and behaviour when a complete capture exceeds them; do not
silently truncate and call it the entire state. Avoid speculative pagination
until needed, but do not remove existing transport safety limits.

## Task 06 — schema/link inspection UI, milestone 2

Add configuration browsing and selected-node policy details using Task 05.
Show match selectors and policy, and navigate from aliases to canonical paths.
Distinguish fetching, successful empty results, unavailable API, failed request
and previous captured result. A failed refresh must not erase policy into an
apparent “no schemas” state. Label refresh snapshots as such and show their
captured node data alongside the explanation.

HTTP inspection is separate from the WebSocket-only SDK. Keep the HTTP client
in the UI initially unless an explicit SDK extension is approved. Derive a
matching default HTTP base from the WebSocket URL using URL parsing, with an
explicit override for proxies/custom routing; never string-replace URLs.
Do not implement TLS/browser support as a side effect of this task.

Real-server acceptance: install a schema, inspect a node, replace policy without
changing its value, refresh, and observe the new rule. Exercise a linked view
becoming disabled/recovering. Confirm configuration matching no nodes remains
browsable. Test unsupported endpoint and transport errors. Retain milestone 1
operation against servers without the new inspection API.

Update US-13 evidence honestly: explicitly refreshed policy is not live policy;
complete public-state coverage requires an inventory of the agreed public
surface. List omissions. Live policy notifications, occurrence logs and editing
are separate follow-up tasks, not hidden requirements of milestone 1 or 2.

## Validation and delivery for every task

Use the pinned `nightly-2026-10-01`. For meaningful behaviour record a behavioural
red test, implementation, and green result. Use focused tests while iterating.
Run formatting, Clippy and relevant tests for each task. At milestones run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --release
cargo build -p tanuki-ui
git diff --check
```

Real transport tests need loopback access. Report sandbox failures separately
from application regressions; request the required execution capability instead
of weakening tests. Keep terminal smoke tests separate from headless CI tests.

Maintain `docs/architecture.md`, `docs/protocol.md`, `docs/procedures.md`,
`docs/toolchain.md`, `docs/testing.md`, README and US-13 where affected. Preserve
stable story IDs. Never claim deployment from simulated-client tests.

After checks, commit and push completed code/tests/docs to `github` under the
standing repository authorization. Report any blocker explicitly. Final report:
behaviour delivered, commands/results, provisional choices, remaining gaps,
commit and push status. Do not merely say “done”.

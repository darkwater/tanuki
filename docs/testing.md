# Testing

## Bootstrap lifecycle

`tanuki/tests/server_lifecycle.rs` checks that the production lifecycle seam accepts
an injected shutdown request and that its task joins successfully.

Run it with:

```sh
cargo test --test server_lifecycle
```

The full quality gates are recorded in `docs/toolchain.md` and CI.

## Task 01 — domain paths, values, and operations

| Contract | Evidence |
| --- | --- |
| Absolute topics; virtual root; empty/dot/trailing-segment rejection | `tanuki/tests/domain_paths.rs::topic_paths_enforce_boundaries_through_every_public_conversion` |
| Reserved selector characters and system namespace | `tanuki/tests/domain_paths.rs::topic_paths_enforce_boundaries_through_every_public_conversion` |
| Exact, `*`, zero-or-more `**`, brace choice, union, and empty matching | `tanuki/tests/domain_paths.rs::selector_matcher_covers_exact_single_recursive_choice_union_and_empty` |
| Selector intersection and nonintersection matching invariant | `tanuki/tests/domain_paths.rs::selector_intersection_detects_shared_possible_topics` and `nonintersecting_selectors_never_match_the_same_topic_in_a_small_corpus` |
| Malformed selectors and codec validation | `tanuki/tests/domain_paths.rs::malformed_selector_syntax_is_rejected_instead_of_guessed` |
| Maps do not create child topics | `tanuki/tests/domain_paths.rs::maps_remain_values_instead_of_implicit_child_topics` and `tanuki/tests/domain_values.rs::a_map_is_one_runtime_value` |
| Finite floats and missing-versus-null desired payload | `tanuki/tests/domain_values.rs` |
| Instant nodes cannot retain payloads | `tanuki/tests/domain_values.rs::instant_nodes_cannot_retain_payloads_or_expiry` |
| Validated client-name deserialization | `tanuki/tests/domain_values.rs::client_name_deserialization_cannot_bypass_validation` |
| Nonnegative timer durations and nonempty batches | `tanuki/tests/domain_operations.rs` |

Run the focused domain suite with:

```sh
cargo test --test domain_paths --test domain_values --test domain_operations
```

Task 01 defines the transport-independent value algebra; task 06 adds
JSON/MessagePack runtime-value equivalence coverage.

## Task 02 — core publication and atomic batches

`tanuki/tests/core_publication.rs` covers:

- reserved-system rejection with no state effects;
- late deadline failure after staged state and event operations, with no
  occurrence, state, or sequence effect;
- one coherent update batch for paired lamp properties;
- identical-value timestamp and expiry refresh;
- warned output takeover with changed provenance;
- instant occurrence delivery without retained payload;
- warned freeform node-kind replacement; and
- sequential repeated-target execution, final retained-shape coalescing, and
  ordered repeated occurrences;
- selection-filtered snapshots; and
- removal carrying the previous visible state.

Run it with:

```sh
cargo test --test core_publication
```

## Task 03 — managed sessions and inputs

`tanuki/tests/core_sessions_inputs.rs` covers the C1 identity boundary and the first
part of L1:

- stateless define-and-submit creates pending desired intent without a session;
- claiming preserves a desired value, and later submission preserves its claim;
- duplicate managed names invalidate old handles and stale cleanup is harmless;
- matching stateless names cannot claim, displace, or impersonate sessions;
- live claim replacement succeeds with a warning;
- commands work without owners and never retain their payload;
- immediate disconnect releases only the claim;
- grace disconnect returns claim-ID-guarded timer work; and
- claimed-definition denial rolls back unrelated staged changes.

Run it with:

```sh
cargo test --test core_sessions_inputs
```

Task 07 executes the grace work and verifies stale guards in the deadline
scheduler suite.

## Task 04 — JSON and basic HTTP

`tanuki/tests/protocol_json.rs` verifies exact large signed integers, rejection of
out-of-range unsigned integers, and escaping literal reserved tag keys.
`tanuki/tests/http_api.rs` verifies common extractor/domain/routing errors, the single
state convenience route, stateless atomic input batches, tagged JSON values,
anonymous selected reads, and a battery publish/read through a real ephemeral
TCP listener.

Run the focused suite with:

```sh
cargo test --test protocol_json --test http_api
```

The v1 HTTP shape follows the accepted checkpoint-B stream/error contract;
incompatible changes remain possible during the initial release review.

## Task 05 — coherent core subscriptions and checkpoint-B DTOs

`tanuki/tests/core_subscriptions.rs` covers atomic snapshot registration, an actual
concurrent registration/write race, filtered atomic batches, overlapping
selector deduplication, explicit empty snapshots, live-only occurrences,
metadata-only upserts, desired clearing, removals, and isolated slow-consumer
closure. `tanuki/tests/protocol_subscription.rs` covers the provisional correlated
snapshot/update shapes. `tanuki-protocol/tests/selected_view.rs` verifies whole-batch local shape
application, occurrence separation, removals, and stale-update rollback.

Run the focused suite with:

```sh
cargo test -p tanuki --test core_subscriptions --test protocol_subscription
cargo test -p tanuki-protocol --test selected_view
```

The core queue is bounded by batch count. Total outgoing-byte accounting remains
the tracked transport limitation.

## Task 06 — WebSocket codecs and mixed-client simulation

`tanuki/tests/websocket_api.rs` starts the production Axum server on an ephemeral
listener and uses real WebSocket and HTTP clients. It verifies snapshot-first
hello acknowledgement, reply/update ordering, JSON and MessagePack delivery,
same-name stateless HTTP isolation, duplicate managed-session closure, and
disconnect claim cleanup. Its simulated-room case covers E1 steps 1–4 with a
laptop, phone, dashboard, lamp controller, motion/location script, and
automation; assertions follow downstream batches rather than stopping at
producer acknowledgements.

`tanuki/tests/protocol_json.rs` pins native MessagePack bytes, rejection of
extension types, former time tags as ordinary maps, nested codec equivalence, and literal JSON tag-map escaping.
Core slow-consumer isolation remains covered in `tanuki/tests/core_subscriptions.rs`.

Run the focused suite with:

```sh
cargo test --test websocket_api --test protocol_json --test core_subscriptions
```

The tests use simulated actors on loopback, not deployed devices or a polished
client SDK. Total outgoing-byte accounting remains unimplemented.

## SSE endpoint

`tanuki/tests/http_api.rs::sse_starts_with_a_snapshot_then_preserves_an_atomic_update_batch`
verifies the production router's `text/event-stream` response, empty initial
snapshot, one-event delivery of a two-topic atomic commit, and fresh-snapshot
reconnection even when `Last-Event-ID` is supplied.
`tanuki/tests/http_api.rs::sse_query_failures_use_the_common_error_shape` verifies
that pre-stream extractor failures retain the common HTTP error envelope.

Run the focused suite with:

```sh
cargo test --test http_api sse
```

## Task 07 — value expiry and claim grace

`tanuki/tests/core_expiry.rs` verifies that equal due times form one commit, state
expiry removes a node, desired expiry clears only its payload, a refreshed
deadline defeats stale work, submissions during grace survive release, and a
stale claim ID cannot clear a reclaim.

`tanuki/tests/scheduler.rs` runs with paused Tokio time and an independently controlled
wall clock. It verifies that moving a deadline earlier wakes the scheduler and
that queued grace work publishes a claim-only update while retaining submitted
intent. `tanuki/tests/websocket_api.rs::explicit_expiry_and_disconnect_grace_flow_through_live_transports`
extends E1 step 5 across real loopback HTTP/WebSocket traffic.
`tanuki/tests/http_api.rs::omitted_expiry_preserves_an_existing_absolute_deadline`
pins the provisional omission rule.

Run the focused suite with:

```sh
cargo test --test core_expiry --test scheduler --test http_api --test websocket_api
```

## Task 08 — best-effort persistence and restart

`tanuki/tests/persistence.rs` verifies coherent versioned MessagePack capture, absolute-expiry
filtering, retained desired definitions/values, absent restored claims and
sessions, instant metadata without payload replay, sequence continuation,
missing/corrupt/version handling, and a failed atomic replacement that leaves
its target intact. `tanuki/tests/persistence_server.rs` verifies orderly save and
restart over real HTTP, corrupt-file backup with empty startup, plus a
production-binary startup smoke test.

Run the focused suite with:

```sh
cargo test --test persistence --test persistence_server
```

## Task 09 — schema policy and installation

`tanuki/tests/schema_validation.rs` covers warning versus deny outcomes, null policy,
inclusive integer/float ranges, string enums, explicit casts with
revalidation, internal overlap, cross-schema overlap, and ordinary-only
matching. `tanuki/tests/core_schema.rs` covers mutation-path casts, atomic denial,
warning diagnostics, installation against existing values, forced
state/desired cleanup, and denial of a schema-governed node-kind change. HTTP
and WebSocket tests prove their write adapters
cannot bypass the same core checks. `tanuki/tests/persistence.rs` proves installed
schemas still deny invalid values after restart.

Run the focused suite with:

```sh
cargo test --test domain_paths --test schema_validation --test core_schema \
  --test http_api --test websocket_api --test persistence
```

## Task 10 — writable linked views

`tanuki/tests/core_links.rs` covers subtree projection, alias-first then canonical
validation/casting, whole-view invalidation, disabled-alias repair, schema
replacement recovery, instant-event recovery without replay, topology
rejection, replacement, and deletion. `tanuki/tests/core_expiry.rs` verifies alias
removal in the same expiry commit. HTTP and WebSocket tests exercise management
and alias writes through real adapters; persistence rebuilds definitions and
status without storing alias copies.

Run the focused suite with:

```sh
cargo test --test core_links --test core_expiry --test http_api \
  --test websocket_api --test persistence
```

## Task 11 — freshness and active diagnostics

`tanuki/tests/core_freshness.rs` covers overdue onset, retained source data, identical
write recovery, desired/instant exclusions, positive intervals, and prevention
of recursive diagnostics when a diagnostic consumer overflows.
`tanuki/tests/scheduler.rs` proves the expected-update interval wakes the real deadline
scheduler. Link tests cover retained disable/recovery conditions, and
persistence tests prove policy survival plus startup derivation.

Run the focused suite with:

```sh
cargo test --test core_freshness --test scheduler --test core_links \
  --test persistence
```

## Initial end-to-end release scenario

`tanuki/tests/websocket_api.rs::simulated_room_actors_drive_downstream_outputs_across_transports`
covers E1 steps 1–4 and 6 with real loopback HTTP/WebSocket actors, including
warning acceptance and linked dashboard invalidation/recovery. The same file's
controlled-time lifecycle case covers step 5. `tanuki/tests/persistence.rs` and
`tanuki/tests/persistence_server.rs` cover step 7: coherent save/restart, cleared
claims, no instant-payload replay, restored policy, and the production binary.

Run every supported gate with:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo test --release --all-targets --all-features
```

## Protocol client acceptance — 2026-11-22

`tanuki-client` is the protocol layer only (the session SDK and its
`native_sdk.rs`/`session_failures.rs` suites were removed with it). It runs
against the production server over loopback (`tanuki/tests/client_connection.rs`)
and in a real browser (`tanuki-client/tests/browser.rs` via `run-browser.py`).

| Behaviour | Coverage |
| --- | --- |
| Hello, mixed codecs, update-before-reply, correlated errors, clean close acknowledged | `client_connection::a_connection_exchanges_hello_update_reply_and_errors_in_both_codecs`; browser `exchanges_hello_updates_replies_and_errors_in_both_codecs` |
| Bytes, exact floats, `i64::MIN`, null and literal tag maps through updates and fresh snapshots in both codecs | `client_connection::both_codecs_preserve_bytes_floats_and_literal_tag_maps` |
| Split halves from separate tasks; an idle split receiver answers pings past the silence window | `client_connection::split_halves_work_from_separate_tasks_and_keep_the_connection_alive`; browser `split_halves_send_and_receive_independently` |
| Replacement surfaces close code 4001 and reason `session_replaced` | browser `a_replaced_managed_session_closes_with_4001` |
| Invalid URLs and oversized outgoing messages fail locally; the connection stays usable | browser `invalid_urls_and_oversized_messages_fail_locally` |
| Native `wss://` reaches the TLS handshake | `tanuki-client/tests/tls.rs` |
| Serde payload conversion | `tanuki-protocol/tests/payload.rs` |
| Regression found by the earlier SDK | `tanuki/tests/persistence.rs::binary_and_tag_looking_values_survive_a_snapshot_round_trip` verifies streaming stored-node decoding without changing persistence format |

Red phase: reading after `close()` failed with
`ResetWithoutClosingHandshake` because the server returned on a client close
frame without flushing the queued acknowledgement. The server now flushes it
(`transport::acknowledge_close`), including for a close sent instead of hello.

## API/protocol review — 2026-10-07

Behavioral regressions reproduced and corrected:

- `tanuki-protocol/tests/value_roundtrip.rs` checks finite float bits through
  JSON and MessagePack, including signed zero, subnormals, maximum finite
  values and deterministic generated samples. Before enabling exact JSON
  parsing, `2.291712365432881e-9` changed by one bit. The real-server native
  codec scenario also covers that value and `f64::MAX` in requests, updates
  and fresh snapshots.
- `wire::tests::untrusted_array_length_does_not_allocate_before_reading_elements`
  previously panicked with capacity overflow. The visitor now grows runtime
  lists only as elements arrive. `truncated_messagepack_array_returns_a_decode_error`
  also exercises a real maximum-length array32 header with no payload.
- `trailing_messagepack_bytes_fail_without_applying_a_partial_message`
  covers the earlier regression where a malformed binary message delivered an update. Shared frame
  decoding now rejects trailing data in direct and session clients and in the
  server. `websocket_api::trailing_messagepack_data_rejects_the_entire_write_without_mutation`
  exercises the production server, verifies no sequence or state change, and
  sends a valid subsequent write to confirm recovery. Shared request fixtures
  cover exact MessagePack message consumption too.

Existing `selected_view` tests cover sequence rejection,
whole-batch atomicity and immutable historical snapshots after removing the
unnecessary per-delta clone from `SelectedView::apply`.

Run the same workspace formatting, all-target Clippy, debug and release gates
listed above. Real transports require loopback socket access; a sandbox bind
denial is an environment failure, not an application regression.

Review completion: all 163 workspace tests passed in debug and release, with
no ignored tests. `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, and `git diff --check`
also passed.


## Server implementation review — 2026-10-07

The regression red phase reproduced accepted dot-segment link names, reserved
canonical nodes restored from disk, a live managed WebSocket surviving server
return, and an idle SSE stream preventing shutdown. A second lifecycle probe
reproduced shutdown hanging once a non-reading SSE peer filled the socket
buffer; it now exercises the one-second forced I/O boundary.

| Contract | Evidence |
| --- | --- |
| Newest session/socket agree after concurrent replacement; stale cleanup preserves authority | `runtime::tests::concurrent_replacements_keep_the_newest_core_session_and_socket_registered` |
| Retained runtime handles cannot admit work after shutdown | `runtime::tests::shutdown_closes_admission_even_while_runtime_handles_remain_alive` |
| Idle SSE, hello-waiting WebSockets, initialized sockets, and non-reading SSE peers drain | `tanuki/tests/server_lifecycle.rs` |
| Dropping the runtime stops timers even with a retained handle | `runtime::tests::dropping_runtime_stops_timers_even_when_a_handle_is_retained` |
| Scheduler cannot mutate after joined shutdown | `tanuki/tests/scheduler.rs::joined_scheduler_cannot_expire_values_after_server_shutdown` |
| Reserved restore data is rejected and backed up | `tanuki/tests/persistence.rs::reserved_canonical_nodes_are_rejected_and_backed_up_on_restore` |
| Restore enforces deny policy and accepts warnings without casts | `tanuki/tests/persistence.rs::restore_checks_denying_policy_without_casting_or_rejecting_warnings` |
| Final save includes acknowledged writes with a live WebSocket peer | `tanuki/tests/persistence_server.rs::final_save_includes_acknowledged_websocket_writes_with_the_peer_still_open` |
| Link-name diagnostic segment invariant | `tanuki/tests/core_links.rs::diagnostic_link_names_reject_dot_segments_before_installation` |

Existing core atomicity, schema/link, transport parity, and codec tests protect
the shared commit finalizer and DTO refactors. Run the workspace debug/release,
formatting, and Clippy commands above. Loopback tests need socket access.

Local library API changes: construct `runtime::Runtime`, retain its owner, and
pass `runtime.handle()` to `transport::router`. Capture an owned
`Core::persistence_snapshot()` before awaiting `SnapshotStore::save`; the store
no longer receives `Arc<Mutex<Core>>`. Public core outcome imports stay at
`tanuki::core::*` despite the private module split.

Server review completion: all 175 workspace tests passed in debug and release,
with no ignored tests. Formatting, all-target workspace Clippy with warnings
denied, and `git diff --check` also passed. No compiler, dependency, protocol
version, or snapshot format version changed.

## Inspector Task 01 — pure model and formatting

`cargo test -p tanuki-ui` runs 11 headless tests in
`tanuki-ui/tests/inspector.rs`, using actual protocol DTOs and SelectedView,
without a terminal or server. These provide partial US-13.A1/A2 evidence:

- explicit empty state and real parent/child node inspection;
- four distinct kinds, separate value types, missing desired payload versus
  Null, absent/prefix nodes, and instant metadata without retained occurrences;
- atomic paired updates, removal without stale details, metadata-only updates
  and legal sequence gaps; out-of-order/invalid-path rejection without mutation;
- ancestor-preserving filtering, collapse, expansion/selection preservation,
  clamped navigation and nearest surviving selection after multiple removals;
- bytes, readable claims/expiry/provenance,
  terminal-safe controls in paths/values/clients, Unicode preview truncation,
  unchanged DTOs and complete detail text for later scrolling.

The initial behavioral red run compiled and failed all seven original scenario
tests against inert APIs (empty rows, missing labels/formatting and selection).
A subsequent regression test showed removal selecting `/f` instead of the nearer
surviving `/c`; reconciliation was corrected without changing the criterion.
All 11 now pass. No live UI, terminal scrolling/restoration, schema enumeration,
or deployment is claimed. Task 02 supplies real-server lifecycle tests; Task 03
supplies terminal backend and interactive smoke checks.

Task 01 completion checks passed on the pinned nightly: `cargo fmt --all --
--check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test
-p tanuki-ui` and `cargo test -p tanuki-ui --release` (11 tests each), `cargo
build -p tanuki-ui`, foundation-only `cargo run -q -p tanuki-ui`, and `git diff
--check`. Workspace debug/release transport suites are reserved for the planned
milestone gates; this task changes only the UI package and workspace membership.

## Inspector Task 02 — live ownership and lifecycle

`tanuki-ui/tests/live.rs` exercises production server/router/runtime/scheduler
logic over ephemeral loopback WebSockets, with simulated producers:

| Contract | Evidence |
| --- | --- |
| Empty and populated listener baselines; later wildcard paths | `empty_populated_and_future_wildcard_topics_follow_the_live_listener` |
| Whole paired lamp batches, duplicate-value metadata refresh and state expiry | `whole_batches_metadata_and_scheduled_expiry_reach_the_model` |
| `/**` includes reserved diagnostics in both snapshots and deltas; explicit system selectors agree | `broad_subscription_includes_exposed_system_diagnostics` |
| A same-named managed session does not replace the stateless inspector; explicit reconnect installs a fresh baseline | `a_same_named_managed_session_does_not_replace_the_stateless_inspector` |
| Server shutdown and failed reconnect invalidate currentness without erasing cache | `shutdown_and_failed_reconnect_preserve_stale_display_and_join_session` |
| Generated local names coexist (stateless connections never replace) | `generated_names_allow_multiple_inspectors_without_replacement` |
| Cancelled idle receive and multiple commits before rendering lose no batches | `cancelled_idle_receive_and_delayed_rendering_do_not_skip_batches` |

App unit tests inject received server messages and connection errors (server
error reports, a 1013 close, clean end), checking stale-state retention and
typed error sources. `live.rs::a_same_named_managed_session_does_not_replace_the_stateless_inspector`
connects the inspector with the same name as a managed session, verifies that
session survives, follows live updates and explicitly reconnects to a fresh
baseline. Model replacement coverage accepts a restarted
sequence, preserves surviving navigation and rejects malformed snapshots without
mutation. The library README example is a compiled doctest.

For the initial behavioral red, the compiled App stub returned success without
connecting and left the status disconnected; the live-baseline test failed its
expected-live assertion. An earlier sandbox TCP bind denial was environment
failure, not behavioral evidence; rerunning with loopback capability established
the real red. The full inspector/SDK debug suites subsequently passed.

Two test-only assumptions were corrected without changing requirements: server
shutdown may deliver a transport reset rather than a clean close, and joining
SDK tasks does not acknowledge server cleanup. Tests now require stale status
and preserve the actual SDK reason for either shutdown outcome, and verify
server cleanup after joined server shutdown. No server behavior was mocked or
altered to satisfy these cases. Expiry/freshness advance an injected wall clock
and wake the real scheduler through a production transport mutation, with
bounded receive deadlines and no sleeps.

Run focused checks with loopback access:

```sh
cargo test -p tanuki-ui -p tanuki-client
cargo test -p tanuki-ui --release
```

Task 01's headless suite is now `cargo test -p tanuki-ui --test inspector`
(12 tests, including replacement); the full UI package also requires sockets.
Since 2026-11-22 the inspector uses the raw `Connection`; its live tests drive
the server with a small protocol-level writer. Public diagnostics are covered; connection nodes are not
exposed by the current server. This remains model/transport evidence for US-13,
not terminal interaction, complete policy coverage or verified deployment.

Task 02 completion: 40 UI/SDK tests passed in debug (including compiled
examples in documentation), and all 24 UI tests passed in release. Formatting,
workspace all-target Clippy with warnings denied, UI build/status-only binary
smoke check and diff whitespace checks passed on the existing nightly pin.
`cargo tree -p tanuki-ui -e normal --depth 1` confirms no production server
dependency. No compiler, server, SDK, transport limit or protocol change was
made; full workspace debug/release milestone gates remain due for Task 03.

## Inspector Task 03 — terminal milestone 1

`tanuki-ui/tests/render.rs` contains six backend tests. They assert meaningful
text and selection style from actual Ratatui buffers: empty state, populated
parent/child selection and metadata, bottom status/hints, Unicode/control-safe
text, all stale states, zero/tiny dimensions, filtering/help/navigation/exit,
system/instant labels, and long wide-character value endings reachable through
scrolling. The fixture reconstructs visible text using symbol-cell widths;
continuation cells are not literal extra spaces. Model/transport tests retain
all four kind and missing/Null/absent distinctions.

CLI unit tests cover defaults/overrides/help and invalid/missing/duplicate
arguments before terminal setup. A cleanup test injects a failed screen-leave
write and requires a subsequent cursor-show attempt. A stalled-handshake test
waits for a real TCP request byte before checking that drawing/receive
cancellation and joined exit remain responsive. The connection job adds only
one oneshot and JoinHandle, not a queue of updates.

Behavioral red evidence: the initial render/input stub compiled and failed all
six backend scenarios; CLI stubs failed both CLI scenarios; the inert reconnect
starter failed expected connecting status. A strengthened long-Unicode test
then exposed unreachable trailing text after End, corrected with terminal-cell
suffix accounting. A bottom-status assertion failed before connection status
was moved to the footer. These corrections preserve the accepted requirements.

Separate interactive verification: an actual terminal run displayed the error
banner/help and q restoration. `python3 tanuki-ui/tools/terminal-smoke.py` uses
actual server/UI binaries with `/lamp = 1` and `/lamp/hue = 120`, validates
filter/navigation/help and resize, and passed q, Ctrl-C key, external SIGINT,
output failure during partial initialization (`/dev/full`) and a temporary Cargo
probe unwinding through the production TerminalGuard. All five compared actual
termios state before/after, with alternate-screen leave and cursor-show commands
required where output was writable. No production panic switches or broad
panic catching were added. Direct-rustc metadata-stub failure was an environment
artifact, resolved by compiling the probe with Cargo. A smoke-fixture failure
was also corrected to account for differential terminal drawing and Escape
sequence parsing; it was not a server/UI regression.

Run the POSIX smoke checks separately from headless CI:

```sh
cargo build -p tanuki -p tanuki-ui
python3 tanuki-ui/tools/terminal-smoke.py
```

This milestone supplies local simulated inspector evidence for US-13.A1/A2.
It does not claim deployed hardware, schema/link introspection, a connection
projection or complete public-state coverage (A3/A4). Next is checkpoint B
(Task 04), a public inspection API design review, not implementation.

Task 03 milestone gates passed on the existing nightly: `cargo fmt --all --
--check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test
--workspace` and `cargo test --workspace --release` (209 tests each, including
doctests), `cargo build -p tanuki-ui`, CLI help smoke and `git diff --check`.
The final actual-binary PTY run passed all five restoration cases after the
footer/layout correction. Provisional UI defaults are 100 ms dirty redraw ticks,
a 60x10 minimum full layout and eight-scalar horizontal key steps. There is no
new compiler gate, server API or deployment claim.

## Browser connection acceptance — 2026-10-31, reduced 2026-11-22

`tanuki-client` and `tanuki-protocol` compile for `wasm32-unknown-unknown` on
the pinned nightly. The browser suite runs real Rust WASM in headless Chrome
against the production server binary, with temporary persistence and assigned
ports. `run-browser.py` waits for the listener readiness log, launches the WASM
runner, then requests SIGINT and joins shutdown. Since the session layer was
removed, the suite covers the connection only (see the table above);
`connection::browser::tests` covers receive size limits, malformed
JSON/trailing MessagePack and clean/abnormal close translation.

Red evidence: before adapters, browser checking failed in Mio/native networking
and native randomness. More substantively, running the direct-connection browser
regression with Gloo's original no-op Sink close failed specifically because a
send after close still succeeded. Explicit socket close/ownership then made the
same test pass. No assertions were weakened to fit the adapter.

Commands (Chrome plus matching ChromeDriver, or configured WebDriver required):

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked
cargo check -p tanuki-protocol -p tanuki-client --target wasm32-unknown-unknown --locked
cargo clippy -p tanuki-client --target wasm32-unknown-unknown --lib --test browser --locked -- -D warnings
cargo test -p tanuki-client --target wasm32-unknown-unknown --doc --locked
python3 tanuki-client/tests/run-browser.py
python3 tanuki-client/tests/run-browser.py --release
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --release
```

Verification (2026-11-22): the browser suite passed 6 tests (2 unit, 4
production-socket scenarios). Originally (2026-10-31, with the session layer)
debug and release each passed 15 tests using Chrome/ChromeDriver 156.0.8078.80. Native
workspace debug/release (210 tests each), both Clippy configurations, formatting, target checks
and the two SDK documentation snippets pass. Local socket checks need permission
to create loopback sockets; the first sandboxed full run stopped at EPERM in the
existing HTTP socket test, then passed with socket access enabled. CI includes a
separate browser job with the same target checks and both browser profiles.
The CI definition is added; hosted execution is distinct from these local results.

Defaults preserve JSON and 1 MiB messages. Browser `wss://`
uses browser TLS behind a proxy; TLS deployment, other engines, workers, Node,
WASI, JavaScript bindings and rendered/deployed dashboards are unverified.
Browser event/network buffers are outside the client's limits.

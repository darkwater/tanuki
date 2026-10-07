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
state convenience route, stateless atomic input batches, semantic JSON values,
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
snapshot/update shapes. `tanuki/tests/client_view.rs` verifies whole-batch local shape
application, occurrence separation, removals, and stale-update rollback.

Run the focused suite with:

```sh
cargo test --test core_subscriptions --test protocol_subscription --test client_view
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

`tanuki/tests/protocol_json.rs` pins native MessagePack bytes, timestamp/duration
extension vectors, nested codec equivalence, and literal JSON tag-map escaping.
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

## Native SDK acceptance — 2026-10-07

All tests below run in `cargo test --workspace` and its release-mode counterpart.
Real-socket tests bind loopback port 0 and use `server::serve_with_core`; run them
with local networking permitted. Scripted peers are restricted to client failure
injection and do not substitute for production-server acceptance.

| Contract/story | Evidence |
| --- | --- |
| SDK-01, U1/T1 | `tanuki-protocol/tests/requests.rs` covers bidirectional request codecs, validated names/paths, null presence and discriminator order; existing protocol/domain/HTTP/socket suites remain regressions |
| SDK-02, T1 | `tanuki/tests/native_sdk.rs::direct_connection_receives_hello_reply_update_and_remote_error_in_both_codecs` covers mixed frame codecs and correlated replies/errors; `direct_empty_producer_selection_exposes_replacement_close_reason_once` covers empty hello and direct close 4001 |
| SDK-03, C3/US-01.A1 | `session_writes_without_listener_polling_and_registers_current_baselines` and `registration_racing_with_commits_loses_no_retained_update_and_observer_drop_is_independent` exercise current baseline registration, interleaving and two consumers |
| SDK-03, T1/C1 | `a_full_raw_queue_reports_lag_once_without_stalling_another_listener` and `replacement_and_drop_end_listeners_but_preserve_weak_handle_lifetime` exercise full queues, fresh registration, close 4001, hello warnings, weak handles and drop |
| SDK-03 failure injection | `tanuki-client/tests/session_failures.rs` covers cancellation/late replies, pending capacity, lost reply/unknown outcome, oversized local failure, uncorrelated errors, duplicate sequences and invalid option limits; `remote_slow_consumer_reason_preempts_a_full_data_queue` injects close 1013 |
| SDK-04, U1 | `tanuki-client/tests/payload.rs` covers Serde structs/enums/options/bytes, literal reserved keys, i64 limits, unsigned overflow, nonfinite values, map-key rejection and decode errors |
| SDK-04/05, US-07.A1/A2 | `tanuki/tests/native_sdk.rs::typed_lamp_handles_keep_claims_explicit_and_batch_rejection_atomic` covers local-only handles, explicit claim/submission, cross-session rejection and rollback of mixed state/event writes |
| SDK-05, US-09.A1/A2/A4 | `observers_deliver_initial_empty_then_atomic_complete_immutable_snapshots` and `observation_keeps_null_missing_kind_errors_and_metadata_changes_distinct` cover whole snapshots, removal, claim-only metadata, null/absent payload and decode recovery |
| SDK-05, US-07.A3/US-01.A4 | `sdk_observers_receive_timer_removal_and_desired_expiry_without_losing_claim` observes production deadline transitions with explicit controlled wall time and real transport |
| SDK-05 latest state/lag | `observe::tests::unread_outputs_coalesce_but_each_input_delta_is_applied` synchronizes on private watch version, without sleeps; `projection_input_lag_is_terminal_even_with_unread_output` forces a full input queue before scheduling the projection |
| SDK-05 clean shutdown | `session::tests::clean_closure_drains_already_admitted_batches` and `observe::tests::clean_projection_closure_delivers_the_last_unseen_snapshot` distinguish clean draining from abnormal termination; `tanuki/tests/native_sdk.rs::standard_stream_observation_is_native_send_and_ends_after_joined_close` verifies standard Stream polling and native Send ergonomics |
| SDK-06, US-01.A1/A2/US-09.A3/A5 | `tanuki/tests/native_sdk.rs::sdk_battery_producer_and_dashboard_combine_with_stateless_phone_http` runs a typed laptop, union-selected dashboard and same-name stateless HTTP phone through the production server |
| SDK-06, occurrences/T1 | `instant_occurrences_remain_ordered_batches_and_are_not_replayed_to_late_listeners` and `both_socket_codecs_preserve_bytes_semantic_time_and_literal_tag_maps_in_requests_updates_and_snapshots` verify occurrence order, late baseline and full-message value preservation |
| Regression found by SDK | `tanuki/tests/persistence.rs::binary_semantic_and_tag_looking_values_survive_a_snapshot_round_trip` verifies streaming stored-node decoding without changing persistence format |

Behavioral red phases caught missing receive behavior, missing projection
baseline, incorrect payload conversion, buffered MessagePack byte decoding in
requests/persistence, and loss of admitted data on clean closure. Missing imports
were not treated as behavioral evidence.

Publisher/dashboard/controller examples are compiled by
`cargo clippy --workspace --all-targets -- -D warnings` and
`cargo build --workspace --examples`. They were also exercised together against
the compiled production binary on an ephemeral loopback port and isolated
snapshot directory: native battery publication, HTTP desired submission,
controller's atomic hue/brightness report, dashboard typed reads, Ctrl-C exits
and final save all passed. This is local simulated use, not hardware/deployment
or Iced verification. The native supported transport profile is plain `ws://`;
TLS, browser/WASM transport and bindings are outside current acceptance.

Final verification: formatting and all-target workspace Clippy passed; all 156
workspace tests (including documentation checks) passed in both debug and release
modes, with no ignored tests. Protocol-only native compilation and package
dependency direction were checked. The browser-target attempt is accurately
recorded in [toolchain.md](toolchain.md); it could not compile without target std.

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
- `session_failures::dropping_session_rejects_queued_writes_as_definitely_unsent`
  synchronizes admission and drop on the current-thread runtime. Before the fix,
  the queued request could be sent or classified as unknown. It now returns
  `SessionGone`, and the peer confirms no write arrived.
- `shutdown_keeps_transmitted_writes_unknown_and_rejects_new_writes_locally`
  confirms the opposite boundary: an already received but unacknowledged write
  retains `OutcomeUnknown`, while writes after termination fail locally.
- `binary_frame_with_trailing_data_terminates_without_delivering_a_partial_message`
  previously delivered an update from a malformed binary message. Shared frame
  decoding now rejects trailing data in direct and session clients and in the
  server. `websocket_api::trailing_messagepack_data_rejects_the_entire_write_without_mutation`
  exercises the production server, verifies no sequence or state change, and
  sends a valid subsequent write to confirm recovery. Shared request fixtures
  cover exact MessagePack message consumption too.

Existing `client_view` and native SDK observation tests cover sequence rejection,
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

# Testing

## Bootstrap lifecycle

`tests/server_lifecycle.rs` checks that the production lifecycle seam accepts
an injected shutdown request and that its task joins successfully.

Run it with:

```sh
cargo test --test server_lifecycle
```

The full quality gates are recorded in `docs/toolchain.md` and CI.

## Task 01 — domain paths, values, and operations

| Contract | Evidence |
| --- | --- |
| Absolute topics; virtual root; empty/dot/trailing-segment rejection | `tests/domain_paths.rs::topic_paths_enforce_boundaries_through_every_public_conversion` |
| Reserved selector characters and system namespace | `tests/domain_paths.rs::topic_paths_enforce_boundaries_through_every_public_conversion` |
| Exact, `*`, zero-or-more `**`, brace choice, union, and empty matching | `tests/domain_paths.rs::selector_matcher_covers_exact_single_recursive_choice_union_and_empty` |
| Selector intersection and nonintersection matching invariant | `tests/domain_paths.rs::selector_intersection_detects_shared_possible_topics` and `nonintersecting_selectors_never_match_the_same_topic_in_a_small_corpus` |
| Malformed selectors and codec validation | `tests/domain_paths.rs::malformed_selector_syntax_is_rejected_instead_of_guessed` |
| Maps do not create child topics | `tests/domain_paths.rs::maps_remain_values_instead_of_implicit_child_topics` and `tests/domain_values.rs::a_map_is_one_runtime_value` |
| Finite floats and missing-versus-null desired payload | `tests/domain_values.rs` |
| Instant nodes cannot retain payloads | `tests/domain_values.rs::instant_nodes_cannot_retain_payloads_or_expiry` |
| Validated client-name deserialization | `tests/domain_values.rs::client_name_deserialization_cannot_bypass_validation` |
| Nonnegative timer durations and nonempty batches | `tests/domain_operations.rs` |

Run the focused domain suite with:

```sh
cargo test --test domain_paths --test domain_values --test domain_operations
```

Task 01 defines the transport-independent value algebra; task 06 adds
JSON/MessagePack runtime-value equivalence coverage.

## Task 02 — core publication and atomic batches

`tests/core_publication.rs` covers:

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

`tests/core_sessions_inputs.rs` covers the C1 identity boundary and the first
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

`tests/protocol_json.rs` verifies exact large signed integers, rejection of
out-of-range unsigned integers, and escaping literal reserved tag keys.
`tests/http_api.rs` verifies common extractor/domain/routing errors, the single
state convenience route, stateless atomic input batches, semantic JSON values,
anonymous selected reads, and a battery publish/read through a real ephemeral
TCP listener.

Run the focused suite with:

```sh
cargo test --test protocol_json --test http_api
```

The v1 HTTP shape is provisional until checkpoint B.

## Task 05 — coherent core subscriptions and checkpoint-B DTOs

`tests/core_subscriptions.rs` covers atomic snapshot registration, an actual
concurrent registration/write race, filtered atomic batches, overlapping
selector deduplication, explicit empty snapshots, live-only occurrences,
metadata-only upserts, desired clearing, removals, and isolated slow-consumer
closure. `tests/protocol_subscription.rs` covers the provisional correlated
snapshot/update shapes. `tests/client_view.rs` verifies whole-batch local shape
application, occurrence separation, removals, and stale-update rollback.

Run the focused suite with:

```sh
cargo test --test core_subscriptions --test protocol_subscription --test client_view
```

The core queue is bounded by batch count. Proposed connection byte limits and
the wire shapes remain provisional pending the user's checkpoint-B answers.

## Task 06 — WebSocket codecs and mixed-client simulation

`tests/websocket_api.rs` starts the production Axum server on an ephemeral
listener and uses real WebSocket and HTTP clients. It verifies snapshot-first
hello acknowledgement, reply/update ordering, JSON and MessagePack delivery,
same-name stateless HTTP isolation, duplicate managed-session closure, and
disconnect claim cleanup. Its simulated-room case covers E1 steps 1–4 with a
laptop, phone, dashboard, lamp controller, motion/location script, and
automation; assertions follow downstream batches rather than stopping at
producer acknowledgements.

`tests/protocol_json.rs` pins native MessagePack bytes, timestamp/duration
extension vectors, nested codec equivalence, and literal JSON tag-map escaping.
Core slow-consumer isolation remains covered in `tests/core_subscriptions.rs`.

Run the focused suite with:

```sh
cargo test --test websocket_api --test protocol_json --test core_subscriptions
```

The tests use simulated actors on loopback, not deployed devices or a polished
client SDK. Total outgoing-byte accounting remains unimplemented.

## Task 07 — value expiry and claim grace

`tests/core_expiry.rs` verifies that equal due times form one commit, state
expiry removes a node, desired expiry clears only its payload, a refreshed
deadline defeats stale work, submissions during grace survive release, and a
stale claim ID cannot clear a reclaim.

`tests/scheduler.rs` runs with paused Tokio time and an independently controlled
wall clock. It verifies that moving a deadline earlier wakes the scheduler and
that queued grace work publishes a claim-only update while retaining submitted
intent. `tests/websocket_api.rs::explicit_expiry_and_disconnect_grace_flow_through_live_transports`
extends E1 step 5 across real loopback HTTP/WebSocket traffic.
`tests/http_api.rs::omitted_expiry_preserves_an_existing_absolute_deadline`
pins the provisional omission rule.

Run the focused suite with:

```sh
cargo test --test core_expiry --test scheduler --test http_api --test websocket_api
```

## Task 08 — best-effort persistence and restart

`tests/persistence.rs` verifies coherent versioned MessagePack capture, absolute-expiry
filtering, retained desired definitions/values, absent restored claims and
sessions, instant metadata without payload replay, sequence continuation,
missing/corrupt/version handling, and a failed atomic replacement that leaves
its target intact. `tests/persistence_server.rs` verifies orderly save and
restart over real HTTP, corrupt-file backup with empty startup, plus a
production-binary startup smoke test.

Run the focused suite with:

```sh
cargo test --test persistence --test persistence_server
```

## Task 09 — schema validation foundation

`tests/schema_validation.rs` covers warning versus deny outcomes, null as a
policy separate from non-null kind, inclusive integer/float ranges, string
enums, invalid validator construction, ordinary-only matching, and rejection
of explicitly system-rooted selectors. `tests/domain_paths.rs` covers selector
intersection and the nonintersection matching invariant.

Core installation and mutation-path enforcement are not implemented yet; they
remain behind the explicit activation/casting questions in
`docs/open-questions.md`.

Run the focused suite with:

```sh
cargo test --test domain_paths --test schema_validation
```

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

JSON/MessagePack runtime-value codec equivalence remains task 06 work; task 01
defines the transport-independent value algebra only.

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
- provisional repeated-target rejection;
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

Executing grace timers, desired-value expiry, and stale timer guards remains
task 07 work.

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

The v1 HTTP shape is provisional until checkpoint B. WebSocket and MessagePack
transport parity remain task 06.

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
the wire shapes remain provisional pending the user's checkpoint-B answers;
WebSocket delivery is task 06.

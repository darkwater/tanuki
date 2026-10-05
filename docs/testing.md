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

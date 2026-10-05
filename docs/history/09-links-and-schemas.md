# Selectors, links, and schema boundaries

Updated 2026-10-05. Requirements, current directions, and proposals are distinguished below.

## Path selectors

User direction: use Unix-style wildcards, e.g. `/battery/*` and `/devices/**`, rather than MQTT wildcard syntax.

Assistant proposals, not yet individually accepted:

- `*` matches within one segment and never crosses `/`.
- `**` occupies a complete segment and matches zero or more segments.
- `{a,b}` selects explicit alternatives; no numeric ranges initially.
- `/devices/**` can include a value at `/devices` itself under zero-segment matching.
- Defer `?` and character classes; provide literal selectors separately.

Exact syntax, escaping, root matching, and structural parents remain open.

## Links and indexes

User direction: provide alternative paths to the same data, and maintain both forward and reverse link indexes. Forward resolution supports initial reads; reverse lookup supports forwarding updates to linked views.

Example: `/battery/laptop` links to `/devices/laptop/battery`.

Assistant proposal: each link has one target; a target may have many incoming links. Reverse indexes are derived from authoritative forward links and updated consistently on creation, retargeting, and deletion. The earlier single-node-only restriction was a simplification proposal; the user now explicitly considers directory/subtree links as well.

Cycle handling, chain support, access permissions, dangling targets, matching through subtrees, and deduplication are unresolved. Subtree reverse lookup must find links targeting ancestors of a changed node; exact-node indexing alone is insufficient.

## Link schema violations — latest user direction

Do not let a schema attached through a linked path block valid changes to the source. Instead, a violation should remove or disable the link. If any child or grandchild of a directory link violates the linked view's schema, invalidate the entire directory link, not just that child.

Two possible recovery models remain open:

1. Remove the link and rely on clients to recreate it.
2. Disable the link, retain its definition, and automatically reinstate it when the violation is resolved.

This supersedes the assistant's earlier suggestion that all alias schemas constrain source writes.

Assistant preference: disable with a visible reason rather than delete. Proposed state:

```rust
enum LinkState {
    Active,
    Disabled { violations: Vec<Violation> },
}
```

Keep dependency/reverse indexes for disabled links if automatic recovery is supported. Reactivation requires checking the whole current linked view, including new or missing descendants where relevant. Relevant schema changes can also cause disablement or reactivation.

## Atomic visibility — proposal

Evaluate the final candidate state of an atomic write batch. A source commit and any resulting link invalidation must be made visible together: consumers must not briefly receive invalid values through the link before removal arrives.

For a directory link, remove all visible descendant entries together in the linked subscription view. Removal events carry the last valid state that was exposed through that view, not the new invalid source state. On recovery, publish the current valid linked view atomically. Initial reads obey the same active/disabled boundary. These details implement the previous atomic-update and no-invalid-consumer-state goals; they remain a proposed mechanism.

## Schema interface and deferred details

User preference: define structural integration now and defer advanced schema language/validators. The core question remains 'is this value valid for this topic?'

## Clarified validation semantics — user direction

A violation through a link is a violation of the link: it no longer points to a value valid at that linked path. It is not a violation of a write that passes direct validation. Linked validation must not reject or mutate that valid source write.

- Direct violation with casting enabled: attempt conversion, validate again, then store the converted value if valid.
- If conversion fails or revalidation fails: apply the configured reject/warn fallback.
- Direct validation passes but linked validation fails: casting is not allowed for linked validation; apply reject/warn to the link.
- The final stored source value, including any successful direct conversion, is the value checked through links.

Assistant policy sketch, not a selected API:

```rust
struct ViolationPolicy {
    try_cast: bool,
    fallback: OnInvalid,
}

enum OnInvalid {
    Reject,
    Warn,
}

fn validate(topic: &NodePath, value: &Value) -> Result<(), Violation>;
```

The call uses configured schemas internally. Link validation skips casting even when the same policy would permit it during direct validation. A cast is not itself acceptance: revalidate before commit. Prefer a bounded conversion attempt rather than an unrestricted retry loop; multiple interacting schema conversions remain an open implementation detail.

## Outcome interpretation and open details

| Context | Reject | Warn |
| --- | --- | --- |
| Direct write | Reject the write/batch and leave stored values unchanged | Proposed meaning: accept with diagnostic |
| Link validity | Remove or disable the violating link, leaving the valid source write intact | Proposed meaning: retain the link with diagnostic |

The user's reject/warn distinction is accepted as a possible fallback policy; exact warning visibility and whether warn retains the violating link still need confirmation. Warn-and-accept cannot carry the earlier unconditional guarantee that consumers only see valid values. Strict reject policies can preserve that guarantee; warning policies must be described as advisory if they permit invalid visibility.

Delete versus disable/reinstate remains open. Atomic visibility rules above apply when the link is invalidated. No alias-specific conversion or transformed linked values are allowed under the clarified model.

Metadata access, structural validation, and whole-batch candidate context remain open. Future cross-node checks may require read-only candidate-tree context even if the caller-facing validation interface stays small. Default policies and schema-language syntax have not been selected.

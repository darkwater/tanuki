# Open questions and review ledger

This is the prioritized handoff list for decisions that still merit user
attention. Provisional implementation is allowed while a question is open;
that does not turn the provisional choice into an accepted requirement.

## Actually important — please answer

1. **Checkpoint B subscription contract.** Should a successful initial
   `snapshot` double as the reply to `subscribe`, followed only by `update`
   messages, with request-correlated `reply`/`error` for other operations?
   The provisional implementation will use this simpler ordering and full-node
   upserts inside atomic delta batches.
2. **Slow-consumer contract and limits.** Is disconnect-and-resnapshot the
   desired policy when a connection exceeds a 1 MiB outgoing-byte budget, with
   no dropped/coalesced occurrences? Also confirm 1 MiB as the initial maximum
   individual snapshot/message size. The core currently proves the policy with
   a configurable batch-count queue; byte accounting belongs at the codec/
   connection boundary.
3. **Repeated same-topic operations.** Should checkpoint B freeze the current
   rule: one definition, one claim, and one submission slot may compose in
   operation order, while repeats in a slot and all same-topic output/removal
   combinations are rejected?

## Consequential, but the likely answer seems clear

1. **Retained node wire shape.** Normalize both state and desired nodes around a
   `current` object (`value`, `last_write`, `expires_at`). Desired `current:
   null` then differs cleanly from `current: {value: null, ...}`. Desired uses
   this shape already; state still has the older top-level fields pending the
   checkpoint-B DTO refactor.
2. **Request identifiers.** Use opaque client-supplied strings. An error for a
   message that cannot be decoded enough to recover its ID uses `request_id:
   null`.
3. **Filtered commit sequences.** Keep the global commit sequence in snapshots
   and updates. Selected streams legitimately skip irrelevant global commits,
   so a gap is informational and not proof of message loss.

## Provisional defaults probably worth a quick skim

1. HTTP listens on `127.0.0.1:3000` unless `TANUKI_LISTEN` is set.
2. Stateless writes use the validated `tanuki-client` header; reads are
   anonymous.
3. Retained writes require an explicit expiry mode until omitted-expiry
   semantics are selected in task 07.
4. JSON semantic tags are `$bytes`, `$timestamp`, `$duration`, `$int`, and
   `$map`; literal maps containing reserved tag keys must use `$map`.
5. Exact current dependency versions are pinned and `Cargo.lock` is committed.

## Resolved or accepted elsewhere

Accepted decisions remain in `docs/architecture.md` and
`docs/decisions-to-review.md`; this file should not reopen them without a new
reason. Remove items above when resolved and record the outcome in the
authoritative document.

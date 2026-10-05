# Open questions and review ledger

This is the prioritized handoff list for decisions that still merit user
attention. Provisional implementation is allowed while a question is open;
that does not turn the provisional choice into an accepted requirement.

## Actually important — please answer

1. **Checkpoint C schema installation and overlap.** May I use the recommended
   rule that installing/replacing a direct schema is atomic and rejected when
   current matching values violate it, without casting stored data; every
   matching constraint must pass, and overlapping casting rules are rejected
   to avoid order-dependent conversion?
2. **Checkpoint C and the system namespace.** Should broad schema selectors
   such as `/**` apply only to ordinary topics while explicit `$`-rooted schema
   branches are rejected? The stricter alternative rejects any selector that
   could intersect system paths, which would make `/**` unusable for an
   ordinary catch-all.
3. **Checkpoint B subscription contract.** Should a successful initial
   `snapshot` double as the reply to `subscribe`, followed only by `update`
   messages, with request-correlated `reply`/`error` for other operations?
   The provisional implementation will use this simpler ordering and full-node
   upserts inside atomic delta batches.
4. **Slow-consumer contract and limits.** Is disconnect-and-resnapshot the
   desired policy when a connection exceeds a 1 MiB outgoing-byte budget, with
   no dropped/coalesced occurrences? Also confirm 1 MiB as the initial maximum
   individual snapshot/message size. The transport now enforces the individual
   limit and uses a 64-complete-batch core queue; total queued-byte accounting
   remains unimplemented.
5. **Repeated same-topic operations.** Should checkpoint B freeze the current
   rule: one definition, one claim, and one submission slot may compose in
   operation order, while repeats in a slot and all same-topic output/removal
   combinations are rejected?
6. **Corrupt persistence at startup.** Should Tanuki refuse to start with a
   typed/logged error when its configured snapshot is malformed or unsupported,
   rather than silently treating it as empty? My provisional implementation
   will fail startup and leave the file untouched; an explicit quarantine/
   recovery command can be added if actual operations need it.

## Consequential, but the likely answer seems clear

1. **Retained node wire shape.** Normalize both state and desired nodes around a
   `current` object (`value`, `last_write`, `expires_at`). Desired `current:
   null` then differs cleanly from `current: {value: null, ...}`. The
   provisional snapshot/update DTOs now use this shape for both kinds.
2. **Request identifiers.** Use opaque client-supplied strings. An error for a
   message that cannot be decoded enough to recover its ID uses `request_id:
   null`.
3. **Filtered commit sequences.** Keep the global commit sequence in snapshots
   and updates. Selected streams legitimately skip irrelevant global commits,
   so a gap is informational and not proof of message loss.
4. **MessagePack semantic extension tags.** Use extension tag `1` for UTF-8
   RFC 3339 timestamps and tag `2` for UTF-8 ISO-8601 fixed durations. Bytes use
   MessagePack's native binary type; ordinary values use native primitives.
5. **Connection codec selection.** Let the hello frame choose JSON text or
   MessagePack binary and require that codec for the rest of the connection.
   This avoids per-message ambiguity and currently returns `codec_changed` for
   a later frame of the other kind.
6. **Instant-output metadata across restart.** Preserve event-node publisher
   metadata but never occurrences; preserve command definitions but never
   command payloads. This keeps the existing snapshot model without inventing
   replay.

## Provisional defaults probably worth a quick skim

1. HTTP listens on `127.0.0.1:3000` unless `TANUKI_LISTEN` is set.
2. Stateless writes use the validated `tanuki-client` header; reads are
   anonymous.
3. Omitted retained-value expiry provisionally preserves the existing absolute
   deadline; a new value has no deadline. It does not renew the prior relative
   duration. Explicit `clear` and `set` remain available.
4. JSON semantic tags are `$bytes`, `$timestamp`, `$duration`, `$int`, and
   `$map`; literal maps containing reserved tag keys must use `$map`.
5. Exact current dependency versions are pinned and `Cargo.lock` is committed.
6. WebSocket subscriptions buffer 64 complete update batches. A second managed
   connection with the same name closes the old socket with private-use code
   4001; slow consumers use retry-later code 1013.
7. Persistence will use one readable versioned JSON snapshot, atomic temporary
   file replacement, a 30-second periodic attempt, and a final orderly-shutdown
   attempt. The default path is `tanuki.snapshot.json`; `TANUKI_SNAPSHOT`
   overrides it.

## Resolved or accepted elsewhere

Accepted decisions remain in `docs/architecture.md` and
`docs/decisions-to-review.md`; this file should not reopen them without a new
reason. Remove items above when resolved and record the outcome in the
authoritative document.

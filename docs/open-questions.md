# Open questions and review ledger

This is the prioritized handoff list for decisions that still merit user
attention. Provisional implementation is allowed while a question is open;
that does not turn the provisional choice into an accepted requirement.

## Actually important — please answer

No unresolved question currently blocks tasks 10 or 11.

## Consequential, but the likely answer seems clear

1. **Queued-byte limit.** Keep the accepted disconnect-only behavior and 1 MiB
   individual message limit. Replace the current 64-batch core queue as the
   primary transport limit with a 1 MiB encoded outgoing-byte budget when the
   transport queue is introduced. Overflow closes only that client with
   `slow_consumer`; reconnect is an ordinary new subscription and receives the
   ordinary initial snapshot, with no special recovery protocol.
2. **Retained node wire shape.** Normalize both state and desired nodes around a
   `current` object (`value`, `last_write`, `expires_at`). Desired `current:
   null` then differs cleanly from `current: {value: null, ...}`.
3. **Request identifiers.** Use opaque client-supplied strings. An error for a
   message that cannot be decoded enough to recover its ID uses `request_id:
   null`.
4. **Filtered commit sequences.** Keep the global commit sequence in snapshots
   and updates. Selected streams legitimately skip irrelevant global commits,
   so a gap is informational and not proof of message loss.
5. **Instant-output metadata across restart.** Preserve event-node publisher
   metadata but never occurrences; preserve command definitions but never
   command payloads.

## Provisional defaults probably worth a quick skim

1. Omitted retained-value expiry preserves the existing absolute deadline; a
   new value has no deadline. It does not renew the prior relative duration.
   Explicit `clear` and `set` remain available.
2. JSON semantic tags are `$bytes`, `$timestamp`, `$duration`, `$int`, and
   `$map`; literal maps containing reserved tag keys must use `$map`.
3. Exact current dependency versions are pinned and `Cargo.lock` is committed.
4. A second managed connection with the same name closes the old socket with
   private-use code 4001. Slow consumers use retry-later code 1013.
5. Persistence uses a versioned MessagePack snapshot, atomic temporary-file
   replacement, a 30-second periodic attempt, and a final orderly-shutdown
   attempt. `TANUKI_SNAPSHOT` overrides the default `tanuki.db` path.
6. The initial schema transport is a complete declaration at
   `PUT /v1/schemas/{name}`. `force:true` removes a structurally wrong-kind
   node; it preserves desired definition/claim only when the kind is valid and
   the current payload is the violation.

## Resolved in the 2026-10-06 review

- A subscription yields one complete initial snapshot and then atomic update
  batches. Reconnection has no special resnapshot/replay state.
- Schema rules never govern `/$*`. Broad selectors such as `/**` range over
  ordinary topics for schemas; explicitly system-rooted schema rules are
  invalid.
- Separately installed schemas do not have overlapping rules.
  `Selector::intersects` now provides the pattern-level predicate needed to
  enforce that invariant. Overlap inside one schema is allowed subject to the
  casting restriction below.
- MessagePack timestamps use the standard extension type `-1`; duration keeps
  Tanuki application extension type `2`.
- Every WebSocket text frame is JSON and every binary frame is MessagePack.
  The hello frame's codec is used for unsolicited snapshots/updates, while a
  request's reply uses that request frame's codec.
- Stateless HTTP writes accept either `tanuki-client` or `?client=`. The
  `X-` prefix is intentionally not used because RFC 6648 deprecated that
  convention. Conflicting forms are rejected.
- The default listener is `127.0.0.1:5167`.
- Persistence is MessagePack. Invalid configured snapshots are copied to an
  adjacent `.bak` (then `.bak.N` if needed), logged, and startup continues with
  empty state.
- Normal schema installation rejects existing deny violations. Explicit force
  installation removes invalid state nodes and clears only invalid desired
  payloads; warning violations remain and are reported.
- Repeated same-topic operations execute sequentially inside the candidate
  state. Subscribers receive only the batch's final retained shape, while every
  instant occurrence remains in operation order.
- Initial explicit casts are string to integer, float, or boolean. More liberal
  conversions can be added later without implicit inference.
- Rules inside one schema may overlap and all matching validators apply.
  Intersecting casting rules are rejected even when their casts are identical.
- A schema rule may constrain node kind. Without such a constraint, implicit
  kind changes keep the accepted freeform warning behavior.
- A write through an alias runs the alias schema first, including an explicit
  cast, then sends that result through the canonical schema and its possible
  cast. Unusual multi-cast outcomes are logged. Direct canonical writes remain
  independent and may disable a rejecting linked view.
- A disabled alias accepts repair attempts. A candidate that passes alias and
  canonical policy commits and re-enables the link atomically; a failure leaves
  it disabled. A later valid instant event re-enables its view and is delivered
  without replaying older events.
- Initial links reject cycles/source-mount overlap, overlapping destination
  mounts, destination collisions with retained nodes, targets reached through
  another link, and links to or from `$` system branches. Link chains remain
  future work.
- Tanuki may design read-only `/$*` system trees as implementation needs
  require, subject to later user feedback. The first shared diagnostic design
  uses `/$diagnostics/**`, retains active conditions, and emits transitions
  without recursively diagnosing diagnostic-publication failures.
- Schema freshness initially applies an optional fixed expected-update interval
  only to retained state and present desired values. Missing desired values and
  instant nodes do not become overdue; an identical accepted write refreshes
  freshness and clears overdue status without deleting data.

Accepted decisions are recorded in `docs/spec.md`, `docs/architecture.md`, and
`docs/decisions-to-review.md`. This file should not reopen them without a new
reason.

# Runtime procedures

## Startup and shutdown

The binary starts a Tokio runtime, initializes structured tracing, binds
`TANUKI_LISTEN` (default `127.0.0.1:3000`), constructs one in-memory `Core`, and
serves the Axum router. Ctrl-C starts graceful HTTP shutdown. Tests bind an
ephemeral listener and inject state, clock, and a one-shot shutdown future
through `server::serve_with_core`.

There is no restore step or persistence background work yet. Later startup
will restore before accepting requests; later shutdown will request a
best-effort save after stopping new work.

Subscription, expiry execution, restore, and link-recovery procedures remain
pending their corresponding implementation cards.

## Output mutation and atomic commit

All implemented output writes pass through `Core::apply`:

1. Reject reserved-system targets and ambiguous repeated operations before
   staging. Distinct define/claim/submit input steps may share a topic.
2. Clone authoritative nodes into a private candidate and compute the next
   sequence without modifying live state.
3. Apply operations in request order. Compute provenance from the supplied
   actor and explicit timestamp. Accumulate typed warnings and observable
   changes privately.
4. If any operation fails, discard the candidate, warnings, state changes, and
   event occurrences. The live sequence does not advance.
5. Otherwise install the whole candidate, advance the sequence once, and return
   one indivisible update batch plus successful-operation warnings.

No network or disk work occurs in this transition. State/event publication,
removal, input definition/claiming, desired submission/clearing, and command
submission all use this path.

## Managed-session replacement and disconnect

Opening a managed session allocates an opaque monotonic session ID. If its
client name is already registered, the new ID replaces the old one and a
warning identifies both IDs. Every managed mutation rechecks the handle against
the registry, so the displaced handle immediately loses authority. Stateless
writes neither register nor displace a managed session, even under the same
client name.

Replacement and current-session disconnect inspect claims owned by that exact
session ID. Immediate policies clear the claim in one atomic update while
preserving definitions and desired values. Grace policies retain the claim and
return guarded release work containing the topic, claim ID, and deadline for
the task 07 timer scheduler. A stale disconnect is a successful no-op and
cannot affect the replacement session or its claims.

## Subscription registration and publication

Subscription registration and snapshot capture occur in one exclusive core
call. Once registered, every node-changing transition projects its completed
`UpdateBatch` through each subscriber's selector union and uses nonblocking
queue sends. The write caller never waits for a consumer.

An empty projection produces no update. A full queue marks that subscription
as `SlowConsumer`, drops its sender, and leaves other subscriptions and the
commit unaffected. Already queued updates remain readable before closure. A
transport reconnect will establish a new subscription and snapshot; no replay
or coalescing is performed.

The minimal client applies upserts/removals to a candidate map, installs it only
after processing the complete batch, and reports event/command occurrences
outside retained state. Global sequence jumps are allowed because unrelated
commits are filtered out.

# Runtime procedures

## Startup and shutdown

The binary starts a Tokio runtime, initializes structured tracing, binds
`TANUKI_LISTEN` (default `127.0.0.1:5167`), restores or constructs one in-memory `Core`, and
serves the Axum router. Ctrl-C starts graceful HTTP shutdown. Tests bind an
ephemeral listener and inject state, clock, and a one-shot shutdown future
through `server::serve_with_core`.

Link-recovery remains pending its implementation card. The production server
restores its configured snapshot before serving and starts one deadline
scheduler for the restored shared core.

## Output mutation and atomic commit

All caller-initiated mutations pass through `Core::apply`:

1. Reject reserved-system targets before staging. Repeated same-topic
   operations are allowed and execute in request order.
2. Clone authoritative nodes into a private candidate and compute the next
   sequence without modifying live state.
3. Apply operations in request order. For every value-bearing operation, run
   the installed ordinary-topic schema first, apply at most one explicit cast,
   and revalidate all matching rules. Compute provenance from the supplied
   actor and explicit timestamp. Accumulate typed warnings and observable
   changes privately.
4. If any operation fails, discard the candidate, warnings, state changes, and
   event occurrences. The live sequence does not advance.
5. Otherwise coalesce each topic's retained changes against its pre-batch and
   final state while preserving all instant occurrences in order. Install the
   whole candidate, advance the sequence once, and return one indivisible
   update batch plus successful-operation warnings.

No network or disk work occurs in this transition. State/event publication,
removal, input definition/claiming, desired submission/clearing, and command
submission all use this path.

## Schema installation

`Core::install_schema` first builds a candidate registry and rejects overlap
with other named schemas. It inspects current stored values without casting
them. Normal installation reports current denying violations and changes
nothing. Forced installation removes invalid state nodes and clears invalid
desired current values while retaining definitions and claims. Warning
violations remain and are returned as successful diagnostics. Cleanup is one
ordinary atomic update; sequence exhaustion leaves both nodes and the registry
unchanged. HTTP installs a complete declaration through
`PUT /v1/schemas/{name}`.

A denying node-kind constraint rejects an incompatible write before staging.
On forced installation, a structurally incompatible node is removed because
retaining that node could not satisfy the new constraint; desired metadata is
preserved only for payload violations where its kind remains valid.

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
the deadline scheduler. A stale disconnect is a successful no-op and
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

## WebSocket connection lifecycle

1. Upgrade `/v1/ws`, then require one text-JSON or binary-MessagePack hello.
2. Open the named managed session and register its selectors at the core's
   snapshot boundary.
3. Install the transport registry entry. If it replaces an entry, signal the
   old socket to close with `session_replaced`.
4. Send the correlated snapshot as the first response, then select among
   client messages, subscription batches, and replacement notification.
5. Apply every write through `Core::apply`; send its correlated reply before
   forwarding its own queued selected update.
6. On close or I/O failure, disconnect the exact session handle. Immediate
   claim release is committed and published; a stale old handle is harmless.
   Grace-release timer work is handed to the deadline scheduler.

Message/frame encoding and decoding never happen while the core mutex is held.
An overflowing core subscription cannot stall other clients and eventually
closes with `slow_consumer`. A reconnect has no replay cursor and receives a
fresh snapshot.

## Expiry and claim-grace execution

After every accepted retained write, the transport signals the scheduler to
rescan. It waits for the earlier of the core's next retained-value deadline and
its queued claim-release deadlines. A newly earlier write or release command
wakes and replaces that wait.

At a wake, the scheduler reads the injected wall clock and calls
`Core::process_deadlines`. The core rechecks each current value deadline and
claim ID. Stale work therefore produces no mutation. All changes currently due
are installed as one commit: expired state nodes are removed, desired payloads
are cleared without losing definition/claim, and due matching claims are
cleared without losing definition/value. Subscribers receive the same typed
upsert/removal batches as for caller writes.

An omitted expiry maps provisionally to `Preserve`. On an existing retained
value it retains the old absolute deadline rather than renewing it; on a new
value it creates no deadline. `clear` explicitly removes a deadline and `set`
computes a new absolute deadline from server wall time.

## Save, restore, and shutdown

Production startup reads `TANUKI_SNAPSHOT` or `tanuki.db`. A missing file means
first startup. The snapshot is versioned MessagePack. Malformed data, invalid
domain values, or an unknown format/version is copied to an adjacent `.bak`
without replacing an earlier backup, logged, and treated as empty state. A
failure to read or back up the file remains a typed startup error. A valid
restore drops expired retained values, all sessions, and all claims, and
restores validated schema definitions before the router becomes available.

Every 30 seconds the server clones one coherent snapshot under the core lock.
Serialization and disk I/O happen afterward on a blocking worker. The writer
creates a unique temporary file beside the target, writes and syncs it, renames
it atomically over the target, then syncs the parent directory. Failure removes
the temporary file where possible, logs the error, and leaves live authority
unchanged. Acknowledgements never wait for or promise this periodic save.

On orderly shutdown, new serving stops and in-flight requests drain. The
periodic worker exits, then a final snapshot is attempted. A final failure is
returned to the process rather than hidden. Crash recovery may lose writes
since the last completed replacement; there is no WAL or replay.

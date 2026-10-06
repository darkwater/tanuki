# Runtime procedures

## Startup and shutdown

The binary starts a Tokio runtime, initializes structured tracing, binds
`TANUKI_LISTEN` (default `127.0.0.1:5167`), restores or constructs one in-memory `Core`, and
serves the Axum router. Ctrl-C starts graceful HTTP shutdown. Tests bind an
ephemeral listener and inject state, clock, and a one-shot shutdown future
through `server::serve_with_core`.

The production server restores its configured snapshot before serving, rebuilds
link projections and status from canonical data, and starts one deadline
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

## Link installation, writes, and recovery

`Core::install_link` checks destination collisions and topology against a cloned
registry, evaluates current target nodes under alias schemas, then publishes the
complete visibility diff. Replacement uses the same path. Removal deletes the
definition and publishes alias removals; removing an absent name is a successful
no-op. The HTTP operations require ordinary stateless attribution.

For a write under a mount, the core validates and casts at the supplied alias,
rebases the operation to the target, then validates and casts canonically. This
resolution still occurs while a link is schema-disabled so a valid repair can
commit. For canonical changes, the core validates each affected retained alias
without casting the source value. A denying violation disables the complete
link and publishes removals containing the last visible nodes alongside the
canonical change. Relevant valid data, a valid later instant occurrence, or a
schema replacement rechecks and can restore the full projection atomically.
Old instant occurrences are never replayed.

The same reconciliation runs for deadline and session-driven node changes.
Snapshots persist canonical nodes and link definitions, never alias copies;
restore rebuilds the registry and initial enabled state before serving reads.

## Freshness and active diagnostics

Schema rules may set a positive fixed expected-update interval. The core derives
the earliest deadline from the last accepted write of every matching retained
state or present desired value, including linked views, and the existing
deadline scheduler wakes for it. Processing an overdue deadline leaves source
data untouched and atomically upserts a state condition below
`/$diagnostics/freshness`. An accepted later write—even with an identical
value—updates provenance, removes that condition, and schedules a new deadline.

Link validation similarly derives `/$diagnostics/links/<name>` while a view is
disabled. Recovery removes it in the repair commit. Diagnostic state bypasses
the ordinary mutation path and is recomputed only from authoritative policy and
data; clients cannot write it and it is not fed through schemas or links.
Publication uses the normal nonblocking subscription queue. A failed or slow
diagnostic consumer is disconnected without publishing another condition, so
failure cannot recurse. Restore recomputes current active conditions from
persisted nodes, schemas, links, and startup time.

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

## SSE connection lifecycle

1. Parse the required `select` query parameter on `GET /v1/sse` using the same
   selector type as snapshot reads.
2. Atomically register an anonymous core subscription and capture its selected
   snapshot.
3. Send that snapshot as one `snapshot` event, then map every selected core
   batch to exactly one `update` event. Send a keep-alive comment after 15 idle
   seconds.
4. On response cancellation, drop the subscription receiver. On bounded-queue
   overflow, drain any already queued batches, log `SlowConsumer`, and end the
   response without stalling mutation.

SSE event IDs mirror global commit sequences but are not replay cursors.
`Last-Event-ID` is ignored, so reconnect always starts with a newly captured
snapshot and never receives historical instant occurrences.

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

## Native SDK operation

Run the ordinary Rust examples with the commands in
[the client README](../crates/tanuki-client/README.md). They use the production
WebSocket endpoint; HTTP remains available for one-off phone/remote submissions.
The controller example defines and claims desired brightness explicitly and
simulates a device report atomically. Its accepted report is not physical-device
verification. Separate processes use distinct example client names.

Retain Session for the intended connection lifetime. Creating/cloning a topic
handle is local and holds no claim or strong session ownership. Dropping an
observer cancels that projection; dropping Session signals native shutdown.
Prefer `close().await` to join driver/writer and projection tasks. It attempts a
normal WebSocket close, waits at most one second for writer shutdown and reports
prior abnormal failure. Server disconnect/grace work still follows the existing
core session lifecycle; client close has no server cleanup acknowledgement.

Raw lag is terminal for only the affected receiver; fresh registration recovers
current retained state without replaying occurrences. Slow snapshot output can
coalesce safely because its projection consumes every delta. Abnormal closure
has a path independent of data queues and reports once; clean close drains
admitted deltas and exposes final unseen latest state. Prior snapshots are
historical immutable values. Replacement code 4001 and slow-consumer code 1013
remain inspectable in connection termination reasons.

Inspect write warnings on success. A local pre-send failure differs from a reply
timeout or lost connection after transmission may have begun; the latter have
unknown outcomes. Cancellation is not rollback. There is no automatic mutation
retry, connection replacement loop or claim recovery in the SDK. Reconnecting
means explicitly creating a new Session with a fresh baseline.

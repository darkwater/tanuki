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

All ordinary caller writes pass through `RuntimeHandle::apply` to `Core::apply`:

1. Reject reserved-system targets before staging. Repeated same-topic
   operations are allowed and execute in request order.
2. Clone authoritative nodes into a private candidate without modifying live state.
3. Apply operations in request order. For every value-bearing operation, run
   the installed ordinary-topic schema first, apply at most one explicit cast,
   and revalidate all matching rules. Compute provenance from the supplied
   actor and explicit timestamp. Accumulate typed warnings and observable
   changes privately.
4. If any operation fails, discard the candidate, warnings, state changes, and
   event occurrences. The live sequence does not advance.
5. Otherwise coalesce each topic's retained changes against its pre-batch and
   final state while preserving all instant occurrences in order. Install the
   whole candidate through the common commit finalizer after checking sequence
   capacity, advance the sequence once, and return one indivisible
   update batch plus successful-operation warnings.

No network or disk work occurs in this transition. State/event publication,
removal, input definition/claiming, desired submission/clearing, and command
submission all use this path.

Schema/link management and timer/session transitions retain their specific
validation procedures and use the same install-before-publication finalizer.

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

The minimal client validates sequence order before applying upserts/removals
under exclusive access and reports event/command occurrences outside retained
state. Callers see the map only after processing the complete batch; no full
cache copy is needed. Global sequence jumps are allowed because unrelated
commits are filtered out.

## WebSocket connection lifecycle

1. Track the stream before upgrading `/v1/ws`, then require one text-JSON or
   binary-MessagePack hello. Shutdown also cancels peers waiting for hello.
2. Open the named managed session and register its selectors at the core's
   snapshot boundary.
3. Under the same runtime coordination lock, install the socket registry entry.
   If it replaces an entry, signal the old socket to close with `session_replaced`.
   No thread can register an older session after a newer one.
4. Send the correlated snapshot as the first response, then select among
   client messages, subscription batches, and replacement notification.
5. Apply every write through `Core::apply`; send its correlated reply before
   forwarding its own queued selected update.
6. On close, I/O failure, or cancellation, the session lease disconnects the
   exact handle. Immediate
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

After every accepted mutation, the runtime signals the scheduler to
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
The core restore boundary rejects canonical system nodes and denying policy
violations without casting, accepts/logs warnings, and checks link topology.

Every 30 seconds the server clones one coherent snapshot under the core lock.
Serialization and disk I/O happen afterward on a blocking worker. The writer
creates a unique temporary file beside the target, writes and syncs it, renames
it atomically over the target, then syncs the parent directory. Failure removes
the temporary file where possible, logs the error, and leaves live authority
unchanged. Acknowledgements never wait for or promise this periodic save.

On orderly shutdown, the runtime closes admission before releasing its
coordination lock, so waiting requests cannot commit after that boundary.
Streams and WebSockets are cancelled, including blocked sends and idle hellos.
WebSocket close delivery and HTTP draining have a provisional one-second
window; remaining socket I/O is then cancelled even if the peer stops reading.
Already admitted commits remain accepted, but their replies may be lost.
Stream leases drain and session leases run exact-handle cleanup; then the
deadline scheduler is stopped and joined. The periodic worker exits and any
in-progress save is joined before the final snapshot is captured. A final failure is
returned to the process rather than hidden. Crash recovery may lose writes
since the last completed replacement; there is no WAL or replay.

## Native SDK operation

Run the ordinary Rust examples with the commands in
[the client README](../tanuki-client/README.md). They use the production
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

Shutdown closes command admission and rejects writes still in the driver
command queue with `SessionGone`: those frames never reached the socket writer.
Writes already handed to that writer retain unknown-outcome semantics. A stop
signal observed before the driver runs takes precedence over queued commands.
Termination is published only after command admission has closed.

## Inspector model procedures — Task 01

Construct `tanuki_ui::model::Model` from a validated-path snapshot. Branches
start collapsed and the first visible row is selected, or the explicit empty
state is shown. Apply one complete update, rebuild rows, prune expansion for
vanished prefixes, and preserve selection by full path. If selection disappears,
prefer its nearest visible ancestor, then nearest surviving old row (preceding
on ties), then first new row. A prefix may survive without a real node; details
then explicitly show no node. Filtering is a case-sensitive path substring and
temporarily exposes matching ancestor context without changing saved expansion.

The model retains cached data when its connection display status changes.
No connection lifecycle, startup/recovery, reconnect, disconnect detection,
expiry scheduler or shutdown tasks are owned by this foundation; Task 02 will
feed their observable effects through snapshots/batches and status changes.
The binary only prints its implementation status and acquires no terminal state.

## Inspector connection procedures — Task 02

Create App with a configured WebSocket URL/client name (defaults: loopback
`/v1/ws`, `tanuki-ui-<pid>-<counter>`). Initially it is disconnected and empty.
An explicit reconnect marks the old cache connecting/stale, removes the old
listener and closes/joins the old session, logging a prior abnormal termination.
It connects once through the native SDK with `/**`, installs a raw listener,
validates/replaces from registration's baseline and then marks the view live.
Failure retains the old data with an error state; there is no retry or replay.

Each `next_update().await` applies one complete batch. Rendering/input can
cancel an idle receive without dequeuing a batch. A clean end marks disconnected
(after SDK drainage); lag marks lagged and other failures mark error. Cache is
retained and the listener removed so later receives remain pending. Explicit
reconnect replaces all retained data, preserving surviving navigation/filtering;
it does not append occurrences. Expiry is a production-server batch, not a UI
timer or polling procedure. Connecting/disconnected/lagged/error all mean that
the display must not be presented as current.

Normal/error exit must await App close to join the SDK session, including
already terminated sessions. Prior abnormal SDK reasons can still be returned
after joining. Client join does not wait for server cleanup acknowledgement;
production server shutdown separately joins its session cleanup. The future
terminal loop must not draw during a network await or hold a server lock while
drawing. Task 03 supplies input/render ticks, controls and restoration; the
current binary still acquires no terminal state.

## Inspector terminal procedures — Task 03

Run `cargo run -p tanuki-ui -- [--url ws://HOST:PORT/v1/ws] [--client NAME]`.
Help and invalid arguments are handled before terminal setup. The default
loopback URL and generated local name are unchanged. Startup enters raw mode,
alternate screen and hidden cursor under an owning guard, starts one explicit
connection attempt and draws the connecting/stale state. Input and redraw stay
active during the attempt. Connection and raw-registration results install
atomically from that listener baseline. Reconnect repeats this procedure with
old-session join; repeated r during an attempt is ignored.

Use arrows/jk to select, left/right to expand/collapse, `/` to edit a filter,
Enter to accept, Escape to clear, Tab to switch pane, PageUp/PageDown for detail
rows and left/right for detail columns. Home/End move to the start/end of the
focused pane's topic list or value columns. `?` opens help. Input is read-only;
q is ordinary text while editing a filter, and Ctrl-C always exits. System
paths are marked; instant nodes explicitly have no retained payload/history.
Schema details are unavailable, never implied to be absent.

Resize is handled through terminal events and Ratatui auto-resizing. Under
60x10, show a resize/status/quit fallback without unsigned size arithmetic
underflow. Render ticks are capped at 100 ms and may coalesce redraws; each SDK
batch still applies. Any non-live state denotes stale cached data. User values,
paths, attribution, filter, URL and error text are escaped before display.

On q, Ctrl-C, terminal EOF or I/O error, drop the terminal guard first, then
cancel/join any connection job and close/join the live SDK session. A prior
abnormal SDK reason may still produce a failing exit code after successful
cleanup. Partial setup errors and Rust unwinding restore via Drop too. No
blocking destructor, panic-catching wrapper or speculative framework is used.
The separate POSIX PTY smoke procedure in `tanuki-ui/tools/terminal-smoke.py`
checks real terminal attributes and actual production binaries in temporary
snapshot storage. It is distinct from headless CI and deployment verification.

## Browser SDK procedures — 2026-10-31

Install `wasm32-unknown-unknown` for the pinned nightly, then run
`cargo check -p tanuki-client --target wasm32-unknown-unknown --locked`.
A Rust browser application depends on the same client crate; Cargo selects its
browser dependencies. Drive application futures through a framework's local
executor or `wasm_bindgen_futures::spawn_local`, without a Tokio runtime. The
[SDK guide](../tanuki-client/README.md#browser-webassembly) includes usage and tests.

Connect/hello waits for the browser open event and correlated snapshot under the
configured deadline. Registration captures current state before forwarding new
complete batches. Writes still pass server validation before commit/publication;
the browser does not become another mutation authority. Errors and local unsent
versus potentially applied outcomes remain visible. Browser timer delays during
backgrounding/suspension do not authorize a retry. Reconnecting is an explicit
new Session with a fresh snapshot, with no occurrence replay.

On application teardown, drop observers and await Session close when possible.
It joins SDK tasks and requests a normal socket close; server disconnect/expiry
procedures run on the server afterward. Drop signals shutdown but cannot make
page unload run queued futures. Browser certificate/mixed-content restrictions
apply; use a TLS proxy for HTTPS deployments. The test helper starts the actual
production binary on an assigned loopback port with temporary persistence, waits
for its readiness log, runs browser tests and then sends SIGINT and joins it.

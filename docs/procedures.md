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
   binary-MessagePack hello within the hello timeout (close 1008
   `hello_timeout`). Shutdown also cancels peers waiting for hello.
2. Open the named managed session and register its selectors at the core's
   snapshot boundary.
3. Under the same runtime coordination lock, install the socket registry entry.
   If it replaces an entry, signal the old socket to close with `session_replaced`.
   No thread can register an older session after a newer one.
4. Send the correlated snapshot as the first response, then select among
   client messages, subscription batches, replacement notification, the ping
   timer and the peer-silence deadline. Any received frame refreshes the
   deadline; expiry closes with 1011 `peer_timeout`. Every outgoing write is
   bounded by the send timeout, and an encoded message above 1 MiB closes with
   1009 `message_too_big`.
5. Apply every write through `Core::apply`; forward every queued selected
   update (including the write's own) before its correlated reply. A `select`
   replaces the core subscription and answers with the new snapshot; a
   stateless hello subscribes without opening a managed session or socket
   registry entry (no replacement) and writes through a stateless context.
6. On close, I/O failure, timeout, or cancellation, the session lease disconnects the
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

An omitted wire expiry maps to `Clear`. Explicit `preserve` retains an existing
absolute deadline of the same node kind rather than renewing it; on a new value
or a kind change it creates no deadline. `clear` explicitly removes a deadline and `set`
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

## Client operation

`tanuki_client_core::Client::builder(url, name)...start()` returns a cloneable
`Client` and an `Events` stream and connects in the background (natively inside
a Tokio runtime). Startup: open, hello with the latest selection, receive the
hello's snapshot (all within the connect timeout, default 10 s), emit
`Snapshot { Connected }`, send the setup batch. Disconnect: pending writes fail
`Lost` (outcome unknown), `Disconnected { error, retry_in }` is emitted, writes
fail `NotConnected` until the next connection, and reconnect attempts back off
from 0.5 s to 30 s with jitter. Expiry of claims across the gap is the
server's (immediate or grace release); the setup batch re-claims. Shutdown:
`close()` sends a normal close, waits up to one second for the server's
acknowledgement, emits `Closed(Requested)` and ends the stream; dropping the
last `Client` does the same. A 4001 replacement or rejected hello ends with
`Closed(Replaced)` or `Closed(Rejected)` and no retry. A reader that falls
`event_capacity` updates behind gets `Snapshot { Resync }` instead of the
dropped updates.

## Protocol client operation

`tanuki_client_core::Connection` is one WebSocket with no background tasks. Send
hello first; keep receiving for the connection's lifetime (the native receiver
answers pings, and the server closes a peer silent for 60 seconds). Split the
connection to receive in one task while sending from others. A normal close is
`close()` followed by receiving until `None`; the server acknowledges it.
Request IDs, reply correlation, caching, reconnection and timeouts are the
application's: after a disconnect, open a new connection, send hello again and
replace any cache with the new snapshot. Nothing is replayed, and the outcome
of a write whose reply never arrived is unknown.

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
An explicit reconnect marks the old cache connecting/stale and closes the old
connection. It opens one `Connection`, sends a stateless hello for `/**`,
validates/replaces from that hello's snapshot and then marks the view live.
Failure retains the old data with an error state; there is no retry or replay.

Each `next_update().await` applies one complete batch. Rendering/input can
cancel an idle receive without dequeuing a batch. A clean end marks disconnected;
any abnormal close (including 1013 for a slow consumer) marks error. Cache is
retained and the connection dropped so later receives remain pending. Explicit
reconnect replaces all retained data, preserving surviving navigation/filtering;
it does not append occurrences. Expiry is a production-server batch, not a UI
timer or polling procedure. Connecting/disconnected/lagged/error all mean that
the display must not be presented as current.

Normal/error exit should await App close to close the connection normally.
Client close does not wait for server cleanup acknowledgement;
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
underflow. Render ticks are capped at 100 ms and may coalesce redraws; each received
batch still applies. Any non-live state denotes stale cached data. User values,
paths, attribution, filter, URL and error text are escaped before display.

On q, Ctrl-C, terminal EOF or I/O error, drop the terminal guard first, then
cancel/join any connection job and close the live connection. Partial setup errors and Rust unwinding restore via Drop too. No
blocking destructor, panic-catching wrapper or speculative framework is used.
The separate POSIX PTY smoke procedure in `tanuki-ui/tools/terminal-smoke.py`
checks real terminal attributes and actual production binaries in temporary
snapshot storage. It is distinct from headless CI and deployment verification.

## Browser procedures — 2026-10-31

Install `wasm32-unknown-unknown` for the pinned nightly, then run
`cargo check -p tanuki-client-core -p tanuki-client --target wasm32-unknown-unknown --locked`.
A Rust browser application depends on the same client crate; Cargo selects its
browser dependencies. Drive futures through a framework's local executor or
`wasm_bindgen_futures::spawn_local`, without a Tokio runtime. Page unload
cannot guarantee a clean WebSocket close. Browser certificate/mixed-content
restrictions apply; use a TLS proxy for HTTPS deployments. The test helper
starts the actual production binary on an assigned loopback port with temporary
persistence, waits for its readiness log, runs browser tests and then sends
SIGINT and joins it.

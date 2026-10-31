# Tanuki — test contracts and system simulation

TDD is a primary implementation requirement. Test observable contracts,
internal invariants and interactions; do not settle for snapshots of whatever
the code currently does. Scenario identifiers below are acceptance requirements
as their features land, subject to the explicitly open policies.

## Story traceability

The [user-story index](user-stories/USER-STORIES.md) connects these scenarios
to user goals. Reference stable story criterion IDs in test names/comments or a
small mapping in `docs/testing.md`. Add focused scenarios where the existing
contracts do not cover a story, such as voice indicators, clipboard propagation,
and independent desktop publishers. Document implemented versus
manual/deferred criteria. Mock-client success does not establish verified
real-world use, and stories must not be weakened to make tests pass.

## Test layers

Future inspector coverage is tracked by
[US-13](user-stories/STORY-13-system-inspector.md). When implemented, exercise
the real server with an inspector client: initial tree, atomic updates, node
kinds, absent versus Null payloads, expiry/removal, and schema applicability
changes. Include schemas without matching nodes and unavailable/stale policy
information. UI acceptance remains deferred; existing dashboard transport tests
do not establish it. No event replay or new permission semantics are implied.

| Layer       | What is real                                                               | What is controlled                         | Main evidence                               |
| ---         | ---                                                                        | ---                                        | ---                                         |
| Unit        | Domain constructors, matching and transition functions                     | Input values and explicit time             | Local invariants and boundary rules         |
| Integration | Core, registry, validation, subscription logic; Axum router where relevant | Clock, persistence failures, bounded queue | Component interactions and atomic effects   |
| End-to-end  | Running server and real HTTP/WebSocket codecs/sockets                      | External mock devices/scripts/dashboard    | Complete producer-consumer system behaviour |

Do not replace the core with canned responses in end-to-end tests. Use a small
number of subprocess smoke/restart tests to cover binary startup/configuration;
most full server scenarios can launch the production server entrypoint
in-process on loopback port 0 with isolated temporary storage.

## Harness design

Provide a small TestSystem with start, stop, client construction, observer
registration and explicit readiness. Grow helpers from actual scenarios; do not
build a simulation framework first.

Pass server wall time into deterministic core transitions or use a narrow
injectable clock. Use controllable monotonic scheduling for expiry tests.
Advancing Tokio time alone does not advance a separate wall clock: the harness
must advance the intended time source consistently. Never use real ten-minute
waits or arbitrary sleeps to establish ordering.

Clients wait for acknowledgements, initial snapshots, or observed commits. Use
bounded timeouts only to detect hangs. To prove absence, drain through an
explicit core/subscription barrier or inspect state after an acknowledged
operation; a short timeout by itself does not prove an event was not delivered.
Do not add a public production barrier endpoint only for tests; use a harness
hook or an already specified ordering point.

Capture diagnostics with structured codes and selected fields, not exact prose.
Include client/topic context without dumping arbitrary clipboard or binary
payloads. Teardown closes sockets, cancels and joins tasks, and removes
storage; tests must run independently and concurrently.

## Named acceptance scenarios

### U1 — Values, paths and kinds

- Missing input payload and submitted Null differ.
- A map remains one topic; nested keys are not discovered by a child-topic
  selector.
- Instant node representations cannot retain a payload or payload expiry.
- Paths/selectors are validated through their own APIs, including
  deserialization.
- Matcher tables cover single segment, recursive selection, brace choices,
  unions, empty result and the selected root/escaping rules.
- JSON and MessagePack map to equivalent runtime values for the supported
  profile, including bytes, time values, numeric boundaries and literal
  tag-looking maps.

### C1 — Ownership and identity

- Output writer replacement succeeds with a warning and changes attribution.
- Input submission changes last-writer provenance without changing its claim.
- Duplicate managed names replace the old session. Old handles lose authority;
  delayed old cleanup leaves the replacement intact.
- Same-name stateless writes do not register, disconnect or impersonate the
  managed session.
- Anonymous reads allocate no named session.

### C2 — Atomic writes

Given an observer and two lamp properties, writing both yields a single
coherent batch. The observer never sees new hue with old brightness because it
processed half a batch.

Given a mixed state/event batch with one denying failure, neither the new state
nor any occurrence reaches observers. The caller gets the correct error;
diagnostics may record the attempt. Once links exist, alias effects and
invalidations share the same boundary. Test the chosen repeated-target policy
explicitly.

### C3 — Snapshot handoff

Arrange a commit at the subscription-registration boundary using controlled
scheduling. The new state appears either in the snapshot or in a following
update, never in neither. Snapshots contain no historical instant payloads. An
empty selection produces an explicit empty snapshot. Overlapping selectors do
not duplicate one topic's change.

### L1 — Input lifetime

Define desired brightness, submit 70 with expiry, advance time to the deadline.
Only the payload clears; definition and claim survive. The consumer receives an
explicit change and sees no current desired value. No automatic physical-light
undo is inferred.

Disconnect the owner with configured grace, submit another value during grace,
then expire the claim. Value and definition remain. Reclaiming restores
ownership without erasing that value. A stale release timer cannot clear a
newer claim. Test immediate release separately.

Submit desired temperature before any controller; create an unclaimed input
with explicit kind. A later controller claims and receives it. Commands without
an owner are accepted and delivered to live observers, if any; reconnecting
observers receive no replay.

### L2 — Expiry scheduler

Refresh a value immediately before its old deadline; processing that old timer
must not delete it. Cover moved-earlier deadlines waking the scheduler,
duplicate writes updating timestamps, explicit clear, and the chosen
omitted-expiry policy. Exercise equal deadlines without depending on
unspecified ordering.

### P1 — Persistence and restart

After an explicitly completed snapshot save, restart into the same isolated
directory. Definitions and unexpired retained values return; session claims are
cleared; instant payloads do not return. Expired values never appear in the
first snapshot. Restored state cannot contain half an atomic batch.

Inject a save failure, observe a diagnostic, and verify the live core still
works. A write acknowledged after the last completed save is allowed to be
absent after simulated crash; do not accidentally test or promise durable
acknowledgements. Back up malformed snapshot data, log the recovery, and start
empty; do not silently discard the evidence or overwrite an earlier backup.

### S1 — Schema policy

The same out-of-range battery value is stored with a warning under a warning
rule and rejected under a denying rule. Test direct cast success, failed cast
fallback and revalidation. Check installation against existing data and
overlapping rules according to selected policies. Reserved system topics use
built-in validation regardless of user rules. An overdue-update rule
logs/exposes overdue status without deleting the value.

Run relevant cases in release mode too; no correctness may depend on debug
assertions.

### K1 — Link invalidation and recovery

Expose a subtree through a stricter linked schema. A valid canonical write
violating that view still commits; the entire link becomes disabled and the
observer receives removals containing last-visible valid states. Repair the
canonical data: the link automatically re-enables and exposes current retained
state. Assert diagnostic transitions and no invalid view leakage.

Write through an enabled alias and verify the selected canonical/alias
validation procedure. Add explicit cases for disabled-alias repair and
instant-event recovery once those open policies are chosen. Do not implement
guessed semantics merely to complete a test.

### T1 — Transport consistency and slow clients

Run common success/error vectors through HTTP and WebSocket; mix JSON and
MessagePack WebSocket clients. Assert common error codes/envelopes, including
extractor failures. HTTP stateless identity remains distinct from WebSocket
session registration.

Force one subscriber queue to exceed its configured bound while another
consumes normally. Assert the chosen disconnect diagnostic, no core stall, and
a fresh snapshot on reconnect. Do not silently coalesce events or split atomic
batches.

## E1 — Simulated room: the main end-to-end scenario

Introduce actors incrementally as features land:

1. A persistent laptop publishes battery state over WebSocket. A phone task
   periodically posts battery over stateless HTTP. A dashboard subscribes to
    `/battery/*` and TV paths.
2. A lamp controller owns desired brightness and subscribes to it. A remote
   client submits a desired value. The mock controller consumes it and
   publishes actual lamp properties atomically. The dashboard consumes resulting
   output.
3. Motion producers publish kitchen/desk/couch state. A mock location script
   consumes their selected states and publishes estimated location; a simple
   mock automation consumes location and submits a lamp input. These are external
   test actors, not an embedded Tanuki runtime.
4. A same-name phone-style HTTP write cannot kick the laptop session. A
   deliberately duplicate managed session does replace it, without old cleanup
   breaking the replacement.
5. Advance controlled time through desired-value expiry and a controller claim
   grace period. Assert the specified input lifecycle and resulting
   observations.
6. Apply warning and denying battery rules and a linked dashboard view.
   Exercise warning acceptance, rejection, link disable and automatic recovery.
7. Complete a best-effort save and restart. Reconnect actors; check restored
   retained state, cleared claims and no replayed commands.

Use deterministic actor logic and explicit expected snapshots/batches. Actors
that consume and publish must be observable in the harness; awaiting only a
producer acknowledgement is insufficient to prove the downstream automation
finished.

## Additional checks and completion

Property tests are useful for codec round trips, matcher behaviour and
sequences of write/expire/reclaim operations. Compare with a small independent
reference model where useful, not the production helper itself. Persist failing
seeds. Fuzzing and concurrency model checking are optional tools for a concrete
remaining risk, not initial delivery gates.

For every implemented scenario, list its test path and command in
docs/testing.md. Default test runs should cover unit and integration tests;
provide an explicit end-to-end command if separating slower tests. Avoid
ignored tests that are never run in CI.

Suggested milestone gates, adjusted to actual Cargo layout:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --release
```

Run additional supported feature configurations where they change behaviour; do
not blindly combine mutually exclusive features. No coverage percentage
replaces evidence that core invariants and complete flows were exercised.


## Server review regression extensions — 2026-10-07

- C1/T1: concurrent same-name opening must leave the newest authoritative
  session and socket registration aligned; stale cleanup preserves it.
- T1: shutdown drains idle SSE clients, WebSocket peers before and after hello,
  and peers that stop reading SSE data. Admission closes before cleanup, and
  no retained runtime handle may commit afterward.
- L2: scheduler shutdown is joined before final capture; advancing controlled
  time afterward cannot apply another expiry.
- P1/S1: restore rejects reserved canonical nodes and denying policy violations,
  accepts warning policy, and never casts stored data. Invalid files retain a
  byte-for-byte backup. Final save includes acknowledged WebSocket writes even
  if that peer remains connected when shutdown starts.
- K1/US-11: dot-segment link names fail validation before a disabled-link
  diagnostic path is constructed; ordinary punctuation and Unicode still work.

## Inspector transport increment — US-13, Task 02

Use a real loopback production server and simulated SDK producers. Verify empty
and populated registration baselines, a write between hello and registration,
new wildcard membership, complete paired changes, metadata refresh and expiry.
Compare broad `/**` selection with explicit exposed system diagnostics; do not
apply schema matchers' ordinary-only domain to subscriptions or invent nodes
for unexposed branches. Record missing public projections as limitations.

Disconnect, replacement and lag must stop claiming cached data is current.
Reconnect uses a fresh listener baseline and removes absent stale nodes, with
no retry or occurrence replay. Inject the exact lag receive result at the UI
boundary deterministically; SDK tests establish queue-overflow guarantees.
Assert receive cancellation and skipped draw opportunities do not skip batches.
Client close joins client tasks; check server cleanup after joined server
shutdown rather than assuming immediate cross-transport acknowledgement.
Terminal layout/input/restoration remain inspector Task 03 acceptance.

## Inspector terminal milestone — US-13, Task 03

Use Ratatui's real TestBackend for visible content and selection assertions:
empty/populated panes, separate kind/type labels, metadata, system/instant nodes,
search/expansion/navigation/help, control-character escaping, long Unicode
values and reachable endings, stale/error banners at the bottom and tiny sizes.
Test CLI rejection before terminal acquisition. Establish a pending WebSocket
handshake using a received-byte barrier, then cancel/join exit without sleeps.

Keep POSIX PTY smoke tests separate from headless CI: run actual production
server/UI binaries with temporary persistence, seed state via HTTP, drive keys
and resize, and compare terminal attributes before/after q, Ctrl-C key, SIGINT,
setup-output failure and Rust unwinding through the production guard. Test
restoration attempts cursor show even if leaving the screen fails. Do not add a
production panic switch or catch all panics. A1/A2 get local inspector evidence;
A3/A4 and deployed hardware remain unverified. Run full workspace debug/release,
formatting and all-target Clippy at this milestone.

## Browser SDK transport — US-09, 2026-10-31

Compile protocol/client for `wasm32-unknown-unknown` and run actual Rust WASM in
headless Chrome against the production server, in debug and release. Cover both
outgoing codecs, mixed wire frames, typed bytes, complete paired state, preserved
registration baseline, removal and immutable history, independent raw listeners,
explicit lag, invalid endpoint/oversized writes, local unsent deadlines, duplicate
name replacement with close reason, and joined close. Run shared projection and
raw drainage tests in the browser too. Unit-check malformed/trailing/oversized
incoming frames and clean/abnormal close codes. A silent socket peer may inject a
stalled handshake for browser timer testing; it does not substitute for the real
server's behavior. Browser TLS deployment, other engines and rendered web UI
remain unverified. See [test evidence](testing.md#browser-sdk-acceptance--2026-10-31).

## TV volume example acceptance — US-03.A5–A8

Run `cargo test -p tanuki-tv-volume` and its `--release` equivalent. XML unit
scenarios select instance 0/Master, reject invalid volume and malformed XML,
resolve namespaced device descriptions/URLBase, and honor granted lease times.
Real-socket tests in `examples/tv-volume/tests/bridge.rs` use production Tanuki
and a simulated HTTP TV: initial/changed actual state, desired SOAP control,
no optimistic publication, invalid requests, foreign SID isolation, renewal,
sequence-gap and renewal-failure exit, claim takeover, and output removal.
These do not certify a physical TV or tablet. Hardware notification behavior,
crash expiry and systemd recovery remain manual checks in the example guide;
US-03's power/HDMI criteria are still deferred.

US-03.A8 adds SSDP response parsing, GET-only device-description fetching,
URL/UDN duplicate handling, unrelated-device filtering, ambiguous/missing-device
errors, stable-ID/friendly-name selection and automatic callback routing/port
selection. Discovery socket tests use an isolated loopback responder, never the
LAN multicast destination. `--discover` is the only command used for read-only
LAN verification: no SDK session, subscriptions or SOAP commands. Media playback
must not be disturbed by live control tests. Bridge tests continue using fake
TVs bound to loopback. Actual callbacks and controls on the Samsung TV remain
manual/unverified.

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

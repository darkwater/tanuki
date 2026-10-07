# Tanuki — decisions to review before implementation

2026-10-05. This review contains accepted decisions explicitly labelled as such, plus remaining choices. Recommendations below are proposed defaults to make review concrete. Not every item blocks starting work: each identifies the phase where it matters.

## Main finding

The architecture is sufficiently clear to start organizing the implementation. The input-lifetime examples have now been accepted: definition, payload, and claim have independent lifetimes. Prefer simple policies and useful logs over additional mechanisms for uncommon edge cases. Schema and link choices can be resolved before those phases; their full languages need not be designed now.

## D1 — Node, payload, and claim lifetimes

**Needed before:** core data model and persistence.

A retained input can be defined but never submitted to, lose its desired value to expiry, and lose its owner on disconnect. These are three independent states. Instant nodes may also have lasting definitions despite never retaining their payloads.

**Accepted from the input-lifetime examples:**

- Input value expiry clears only its payload; the definition and claim remain. Expiry does not inherently reverse application effects.
- Claim release preserves the input definition and any unexpired value. Disconnect grace allows submissions to continue; reconnecting controllers explicitly reclaim and receive current intent.
- Claiming/defining an existing input does not erase its submitted value.
- A submission can precede its controller, creating an unclaimed input with its retained/input kind specified.
- Commands without an owner are allowed; they go to current subscribers or are lost. No replay or execution guarantee is implied.
- Restore input definitions and unexpired retained values after restart; clear session claims. Instant commands are gone.
- On a freeform topic with no applicable schema, an implicit node-kind change is accepted with a warning. Incompatible state belonging to the old kind is discarded. Applicable schema policy may deny the change atomically.

**Remaining small policy choice:** instant-output metadata persistence.
Ordinary output publication creates its corresponding kind when absent. The
user's acceptance of the examples does not implicitly approve every earlier
recommendation in this file. Prefer simple documented behaviour, warning on
recoverable anomalies, and logging over added lifecycle machinery.

**Provisional task-02 default:** retain instant-output node metadata, including
the last publisher, while never retaining occurrence payloads. This is an
implementation choice awaiting protocol review rather than an accepted product
decision.

**Provisional task-07 default:** omitting expiry preserves an existing absolute
deadline and creates no deadline for a new value. It does not silently renew
relative time. Explicit `clear` and `set` remain available. This implements the
earlier recommendation but remains listed for user review.

**Accepted claim collision:** a live managed session may replace an existing
input claim with a warning. The displaced session immediately loses claim
authority; stale cleanup remains guarded by internal session/claim identity.

Rust storage should distinguish `Option<Value>` (no payload yet) from `Some(Value::Null)` (a submitted null). That is separate from encoding an optional value inside a schema.

## D2 — Attributed HTTP requests versus managed sessions

**Needed before:** HTTP mutations and session registry.

**Accepted:** basic one-off HTTP writes are attributed but sessionless. They do not register a managed connection and cannot displace same-name managed clients. Anonymous reads remain sessionless. Only explicitly managed sessions participate in duplicate replacement and session-owned claims.

Separate client name from optional session identity in the core mutation context. A matching name on a stateless request does not grant the authority of an existing managed session. The exact API for operations requiring a session remains to be designed.

Decide how connection names map to path segments. Names with spaces are already valid examples. **Recommended initial restriction:** allow spaces and Unicode, reject `/`, control characters, and wildcard syntax; return the original name as metadata. Do not accidentally treat a supplied name as a topic expression.

## D3 — Schema errors, warnings, and installation

**Needed before:** schema enforcement; reserve structured errors in the core now.

**Accepted:** a schema chooses warning (allow and log) or error (deny). Consumer-validity guarantees apply to denying constraints. Prefer permissive, diagnosable behaviour outside explicitly denying rules; built-in system restrictions remain mandatory.

A linked warning can leave the view visible with a diagnostic; a rejecting linked violation disables the link. D4 records the accepted linked-write order and recovery policy.

Other structural choices:

- **Installation:** recommended atomic installation; reject a new direct schema if current matching values violate it. Do not silently cast existing stored data during installation. New linked violations follow link invalidation rules.
- **Overlaps:** recommended all matching constraints must pass. Initially reject overlapping casting rules at installation, then validate any cast result against all applicable rules. This avoids order-dependent conversion.
- **Missing values:** recommended validate values that exist; a definition-only input is not implicit null. Requirements about topic existence can be a future feature.
- **Reserved namespace:** a schema selector `/**` could match system paths. Recommended resolve schema selectors strictly within the ordinary-topic domain, while rejecting explicit `$`-rooted branches. The alternative is to reject any selector that could intersect system paths, including `/**`. Choose one; reads and schemas need not use the same matching domain.
- **Configuration:** recommended submit a complete schema declaration atomically, even if its readable representation has several system child topics.

The exact range/record/enum schema syntax can follow these decisions.

## D4 — Links as views and their failure policy

**Needed before:** link implementation. Writable links and automatic recovery are accepted requirements.

Forward and reverse indexing is clear, but it does not decide the externally visible behaviour.

**Accepted initial profile:**

- Links support writes. Apply the alias schema and its cast first, then translate
  and apply the canonical schema and its cast. Either denying failure rejects
  the whole write. Log unusual multi-cast outcomes.
- A write through a schema-disabled alias is a repair attempt. Passing both
  policies commits and re-enables it atomically; failure leaves it disabled.
- Disable a link on a rejecting direct-source violation, preserve its
  definition, and automatically re-enable when valid again. Log both
  transitions; restored retained views expose current state, not event history.
- Reject cycles, overlapping link mounts, and collisions with existing destination nodes/subtrees. Initially reject targets reached through another link, so link chains need not be supported yet.
- Reject links to or from the reserved system namespace initially.
- Commit source changes and all affected view removals as one batch. Do not leak the invalid candidate value as a removal's “last value”.

**Accepted event case:** an invalid instant event has no stored value to repair
later. Suppress that occurrence through the rejecting view and disable the
link; the canonical occurrence and other valid views still deliver it. A later
valid occurrence re-enables the link and is delivered without replay.

Subtree links also require clear behaviour when the target disappears and returns. Recommended keep the link definition, expose no absent target data, and re-evaluate on target creation while the link remains enabled. Schema-disabled links must be rechecked when relevant target data or schemas change, so they can recover automatically. Manual administrative disabling is not specified.

## D5 — Runtime values and lossless-enough codecs

**Needed before:** publishing a stable wire contract or persistent snapshot format.

The choices are JSON plus MessagePack, inline bytes, semantic timestamps/durations, and ISO 8601 duration text. The full value algebra and tag escapes are not settled.

**Accepted first profile:** null, bool, signed 64-bit integer, finite 64-bit float, string, bytes, list, string-keyed map, timestamp, and signed fixed duration. Unsigned integers and calendar-relative spans are excluded initially; a nonnegative schema range expresses ordinary unsigned constraints without adding a second integer representation. Timer parameters use a distinct nonnegative fixed-duration type.

For JSON, the accepted first mapping is `{"$timestamp":"..."}`,
`{"$duration":"..."}`, `{"$bytes":"<base64>"}`, and
`{"$int":"<decimal>"}` for exact large integers. Literal maps containing
reserved tag keys use `{"$map":{...}}`; ordinary JSON objects remain maps.

MessagePack uses native primitives and binary data, standard timestamp
extension type `-1`, and Tanuki application extension type `2` containing the
fixed-duration text. Both decoders produce the same runtime values.

**Duration ambiguity:** ISO 8601 includes calendar quantities such as months, which do not define a fixed expiry interval without an anchor and calendar policy. Recommended permit calendar spans as values if desired, but restrict expiry/grace/freshness intervals to nonnegative fixed elapsed durations. Decide whether calendar spans belong in the first runtime type at all. These timer policies do not need every capability of a general time library.

## D6 — Atomic batches and subscription shapes

**Needed before:** core commit and WebSocket delivery.

**Recommended first contract:** a single serialized commit produces a single update batch per affected subscription. Register subscriptions and capture their snapshot at one commit boundary; then queue newer batches. Events are occurrences inside a batch, not stored snapshot payloads.

- Execute repeated canonical targets in request order against the private
  candidate. Coalesce retained changes to the pre-batch versus final state and
  preserve all instant occurrences in order. Consumers still apply the entire
  batch before exposing a shape.
- Support state and event operations in one transaction: validate the candidate changes first, then commit and deliver all or none.
- Deduplicate a topic matched by several selectors. Initially treat all startup selectors as one union subscription; multiple independently shaped groups can be a client-library feature.
- Deliver deltas and build coherent shapes in the client first. Do not deserialize the whole object between individual changes within a batch.
- Distinguish node upsert, metadata/payload-state change, event occurrence, and node removal. Snapshot messages are explicitly marked, including an empty snapshot.
- Return request-correlated success/error replies. An acknowledgement means accepted by the core, not executed by a command consumer or durably saved.

A disconnect after commit but before acknowledgement leaves the caller uncertain. No deduplication/replay subsystem is proposed: retrying a command can execute it twice. Document this rather than imply exactly-once delivery.

Task 03 originally allowed only distinct input lifecycle steps on one topic.
The 2026-10-06 review replaced that restriction with general sequential
same-topic execution and final-shape coalescing; no duplicate-target error is
part of the accepted batch contract.

Task 05 implements the accepted snapshot-then-update stream as delta batches with full-node upserts, distinct
event/command occurrences, removals carrying previous nodes, and a correlated
initial snapshot serving as subscribe success. Core queues disconnect only the
slow subscriber on capacity overflow. Reconnection is an ordinary new
subscription, not a replay or special recovery operation. Exact queued-byte
accounting remains open in `open-questions.md`.

Task 06 exposes that shape through JSON-text and MessagePack-binary WebSockets.
The hello frame selects the codec for unsolicited messages, and its correlated
snapshot is always the first server message. Each later frame selects JSON by
text or MessagePack by binary, and its correlated reply uses that same codec.
MessagePack uses standard timestamp extension type `-1` and application type
`2` for fixed-duration text. Axum enforces 1 MiB inbound messages, the encoder refuses larger
individual outbound messages, and subscriptions buffer 64 complete batches;
total queued-byte accounting remains open.

## D7 — Paths and glob grammar

**Needed before:** topic keys and selector parser.

**Accepted initial grammar:** absolute UTF-8 paths, no empty segments or
trailing slash except root, and reject `.` and `..` segments. `*` is a
whole-segment wildcard; `**` is allowed only as an entire segment and matches
zero or more segments. Thus `/devices/**` includes `/devices` itself if it
holds a value. Support finite non-nested whole-segment brace alternatives. Do
not add shell expansion, filesystem lookup, regex behavior, or a textual union
operator; a `Selection` is a collection of selectors.

**Accepted escaping/reservation rule:** ordinary topic segments reject `*`,
`?`, `[`, `]`, `{`, `}`, and `\`, reserving common shell-style selector syntax
rather than introducing an escaping grammar. A first segment beginning with
`$` is the Tanuki system namespace. Other punctuation remains available.

Ordinary paths may have both a value and children; they are not actual
filesystem directories. `/` is an accepted virtual root and is not writable.

## D8 — Operational defaults that should be explicit, not elaborate

**Needed before:** exposing the first running server; these can be small implementation choices.

- **Slow consumers:** recommend bound queued bytes and disconnect on overflow, reporting the reason. Reconnect obtains a new retained snapshot. Do not silently drop events or split batches.
- **Limits:** configure maximum message/value/batch size and subscription queue bytes. Specific numbers can be implementation defaults. A snapshot exceeding the budget should fail explicitly; do not return a silent partial snapshot.
- **Persistence:** use in-memory authority plus periodic coherent snapshots,
  atomic file replacement, and an orderly-shutdown attempt. Include schema/link
  definitions consistently when implemented. A failed save reports
  diagnostics. Invalid startup data is preserved in a backup and logged before
  starting empty, so corruption does not silently masquerade as a new database.
- **HTTP:** recommend POST for mutations; GET for reads. A convenience `value=foo` means string; typed values use an explicit JSON body or encoding option, never heuristic coercion. Use the same batch operation path internally.
- **Exposure:** recommend initial loopback or Unix socket operation, with remote exposure explicitly configured. Full authentication can wait; names are not credentials. Smartphone access still needs a chosen network deployment arrangement.
- **Diagnostics:** reply to callers with structured errors/warnings and log locally from day one. Add observable system diagnostics without recursively publishing failures of the diagnostic mechanism. A future script runtime is not required for this.

## Corrections made during consolidation

- “Last writer owns” applies to outputs, not input submissions.
- “Every request needs a client name” does not apply to sessionless reads.
- A map value never expands into child topics.
- Missing payload is not `Value::Null`, and value expiry is not claim expiry.
- Earlier blob offers/transfer registries are deferred, not initial requirements.
- Schemas do not run on reserved system paths, even if ordinary validation would accept the value.
- MQTT is an optional adapter, not the central broker architecture.
- MessagePack replaces earlier postcard suggestions as the intended binary wire format.
- Best-effort persistence does not imply replay or durable acknowledgements.
- Old assistant recommendations remain recommendations until accepted; this review does not silently settle them.

Task 08 implements persistence as versioned MessagePack, a 30-second periodic
attempt, atomic same-directory replacement, and a final orderly-shutdown
attempt. Invalid configured data is copied to a non-overwriting adjacent
backup, logged, and treated as empty state. The default `tanuki.db` path and
`TANUKI_SNAPSHOT` override are accepted operational defaults.


Server review provisional operational default (2026-10-07): shutdown first closes
mutation/session admission and cancels live streams. HTTP draining and
WebSocket close delivery have a one-second window, then remaining socket I/O is
cancelled. Session cleanup and scheduler termination precede final snapshot
capture. A reply can be lost after an accepted commit; existing unknown-outcome
semantics continue to apply. This is a small implementation default for review,
not a new delivery guarantee or a permissions policy.

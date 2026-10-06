# Tanuki — consolidated specification

Consolidated 2026-10-05 from the design conversation and numbered notes.

This document records the agreed direction. Items explicitly marked **open** or **recommended** are not decisions. Decision IDs refer to [decisions-to-review.md](decisions-to-review.md). The numbered notes preserve discussion history; this document supersedes their conflicting proposals.

## Guiding principles

Keep the system simple and enjoyable to hack on. Accept reasonable edge cases rather than adding machinery for every failure scenario. Tanuki should support robust applications without making ad hoc use cumbersome.

Use logging liberally: unexpected ownership changes, unusual behaviour, link disabling/recovery, and operational failures should be observable with useful context. Prefer warnings and continued operation for recoverable misuse unless a denying schema requires rejection. This does not weaken atomicity, parsing requirements, or built-in system-namespace restrictions.

## Engineering requirements

Implement in Rust on nightly, with behaviour-first TDD at unit, integration and end-to-end levels. Model internal logical concepts with appropriate types and enums; make impossible internal combinations unrepresentable where practical, otherwise guard them at their owning API boundary. Schema enforcement is a higher-level concern and is not this meaning of representational validity.

Use custom error enums (prefer thiserror; no anyhow), idiomatic ecosystem facilities, rustfmt and Clippy. Axum is the preferred web stack, with common response/error types and reusable extractors/middleware. Check current standard and crate APIs before writing complicated workarounds; unstable APIs are welcome when they improve expression. Prefer native async traits, checking current support for required bounds on the chosen nightly.

Use side-effect-free debug assertions for feasible redundant checks without reshaping the architecture; keep primary enforcement effective in release builds. Keep the user involved through focused local type/API reviews and maintain module, data-flow and procedure documentation. Details are in [AGENTS.md](../AGENTS.md), [design.md](design.md), and [test-plan.md](test-plan.md).

## Purpose and boundaries

Tanuki is a central fabric for scripts, devices, and interfaces to exchange named state and events. Ad hoc publication should remain easy. Schemas, metadata, subscriptions, and diagnostics add structure where useful.

Smart home is a primary use case, alongside batteries, desktop state and clipboard, TV state, voice-assistant status, and inferred location within a room. No mandatory device/entity ontology is required. A device can have many producers; one producer can represent many devices.

The core owns semantics. HTTP, WebSocket, and future adapters expose the same operations and validation. Tanuki need not contain a fully compliant MQTT broker. MQTT is a possible later interface.

An embedded script runtime is a future possibility, not a prerequisite. Initial users can run scripts externally. Large blobs, transfer offers, direct peer-to-peer transfers, and streams are deferred; small inline binary values are in scope.

## Topic tree and values

A topic holds one complete value and associated metadata. A map value never creates child topics. `/desktop/state` containing a map and `/desktop/state/clipboard` are separate addresses.

Freeform topics accept changing runtime value kinds, optionally warning about a change. There are only two enforcement modes: freeform, or governed by a schema. There is no intermediate permanent type lock.

Runtime values include ordinary scalar/collection data, null, inline bytes, and first-class timestamps and durations. `Value::Null` represents explicit null; schemas carry optionality/type constraints. Missing topics and inputs with no submitted value are distinct from a present null.

The initial runtime profile uses signed 64-bit integers and finite 64-bit
floats. Unsigned integers are initially omitted: nonnegative schema ranges cover
ordinary unsigned constraints, while values above `i64::MAX` have no initial
representation. Timestamps and signed fixed durations are semantic values;
timer parameters use a separate nonnegative fixed-duration type. ISO 8601
duration text is the agreed wire direction. Calendar-relative spans are
deferred. Enums represented by schema-constrained strings or tagged maps are
recommended, not yet selected as a final contract.

## Four node kinds

Node kind has two independent semantic dimensions: retained/instant and input/output. All four combinations exist. This does not prescribe two Rust bool fields: use meaningful enums or variant-specific node types to express the internal model.

| Kind | Value behaviour | Ownership |
| --- | --- | --- |
| Retained output: state | Stores latest value | Last accepted writer |
| Instant output: event | Emits each occurrence; no retained payload | Last accepted publisher |
| Retained input: desired | Stores latest submitted value | Defining/claiming client, independent of submitter |
| Instant input: command | Emits each submission; no retained payload | Defining/claiming client, independent of submitter |

Output publication implicitly establishes ownership. A different writer normally produces a warning, rather than rejection. Input submission never takes ownership; input definition/claiming is a separate intent. Inputs may be unclaimed. The owner sets input metadata; other clients supply values.

On a freeform topic with no applicable schema, an operation that selects a
different node kind replaces the existing node and produces a warning. State
that is invalid for the new kind, such as a retained payload, input definition,
or claim, is discarded. An applicable schema may deny the change, in which case
the containing atomic batch has no effects.

Illustrative operation names, not a frozen Rust or wire API:

```rust
enum Retention { Retained, Instant }

enum Operation {
    PublishOutput { topic: Path, retention: Retention, value: Value },
    DefineInput { topic: Path, retention: Retention, options: InputOptions },
    SubmitInput { topic: Path, value: Value },
}
```

Definitions and retained payloads are separate. Input value expiry clears the payload while preserving the definition and claim; application logic decides what an absent request means, and expiry does not undo effects. Claim release preserves the definition and any unexpired value. Defining/reclaiming an input preserves pending intent. Submitting before a controller exists can create an unclaimed input, with its retained/input kind specified.

Commands may be submitted without an owner. They reach current subscribers or are lost; acceptance does not promise execution. Persist input definitions and unexpired retained values, but clear session claims on restart. Reconnecting controllers reclaim explicitly.

Repeated writes of an identical value still update `last_updated`, provenance, and supplied expiry. Scripts decide how desired values translate to outputs; Tanuki does not provide built-in desired/actual reconciliation.

## Clients and managed sessions

Clients identify themselves with a supplied name, such as `phone tasker battery script`. These names are attribution labels, not authentication credentials.

An adapter starts a managed session when relevant. Managed sessions are exposed under `/$connections/<connection_name>/...`. Duplicate names follow the shared warn-and-replace/kick policy. Internal unique session identity must prevent displaced sessions or their cleanup from affecting replacements.

A physical HTTP connection is not automatically a managed session. Read-only endpoints can operate without a name or managed session. Basic one-off HTTP writes carry attribution but do not create a managed session and cannot kick off duplicate clients. Client name and optional session identity must be separate in the mutation context (D2).

Persistent sessions can expose connected/disconnected status. A one-off phone submission must not imply that the phone is offline after the request ends. Topic owner, last writer, and the session responsible for a claim are different concepts.

Input claims belong to an owning session. Disconnect either releases them immediately or starts a configured grace period. Other clients' submissions do not renew that claim. Release must be guarded against an intervening replacement claim. Restart clears session claims; reconnecting controllers explicitly reclaim without erasing pending values (D1).

A live managed session may replace an existing input claim with a warning. The
displaced session immediately loses claim authority. Stateless attribution,
including a matching client name, cannot claim or replace a claim.

Transport liveness detection uses ordinary transport mechanisms. A separate application heartbeat or session-resumption protocol is not initially required.

A local Unix-socket entrypoint accessible to permitted local processes is desired. A general authentication/permissions framework is not required initially. Initial exposure and connection-name encoding are implementation-plan decisions (D2, D8).

## Writes and atomic observation

All transports use shared core rules. A write request can affect multiple topics atomically—for example, lamp hue and brightness. Consumers must not observe an intermediate partial result.

A rejected atomic operation must not leave partial state or emit some of its
events. Repeated same-topic operations execute in request order against the
private candidate. Subscribers receive only the final retained node change for
that topic, so a create followed by removal has no visible retained effect;
instant occurrences remain ordered in the atomic batch. Linked-view
invalidation must be part of the same observable change as its triggering
write. Aliased targets through future links still need D6/D4 handling.

Ownership warnings do not themselves reject ordinary output writes. Built-in system restrictions and rejecting schemas do reject prohibited operations. Warning details and error correlation must be available to callers.

## Subscriptions

Selections can cover wildcard paths, chosen branches, or arbitrary unions:

```text
/battery/*
/devices/**
/smart-home/entities/{sensor-kitchen,sensor-desk,sensor-couch}/motion
/voice-assistant/status + /tv/** + /desktop/workspace
```

`*` is a whole-segment wildcard; `**` is a whole-segment recursive wildcard and
matches zero or more segments. Non-nested whole-segment brace choices are
supported. Selector unions are represented as a collection rather than a `+`
text operator. Ordinary topic segments reject the reserved selector characters
`*`, `?`, `[`, `]`, `{`, `}`, and `\`; there is no initial escaping grammar.
The same selector language is shared by all adapters.

Clients can supply subscriptions at startup, including an empty set for producers. They receive a complete initial snapshot, then live updates. The intended implementation must establish the snapshot and subsequent stream without a gap. Instant payloads are not replayed in a snapshot.

Consumers should be able to recompute outputs from a coherent selected input shape. Whole-shape delivery versus a client library applying deltas remains an implementation choice (D6). A Rust wrapper can expose metadata and sending capabilities, while a plain deserialized type exposes the value. A wrapper does not imply different underlying subscription semantics.

Updates are batches. Removal must be a distinct enum variant, preventing accidental use as a current value. A removal carries the last visible state; metadata-only changes and definition-only nodes must also be representable. Exact types remain open.

SSE, when implemented, must preserve atomic batches in one event. No event replay or missed-event log is required. A slow consumer is disconnected without a special recovery state; reconnecting establishes an ordinary new subscription with its initial snapshot.

## Expiry, freshness, and persistence

A relative expiry supplied by a client becomes a server timestamp deadline. `last_updated` and persisted deadlines use wall-clock timestamps; monotonic timers can schedule waits. Duplicate writes are the way to refresh expiry; no distinct refresh operation is necessary.

Expiry scheduling follows the earliest deadline, wakes when the schedule changes, and rechecks current identity/deadline before applying an expiry. An old timer must not delete refreshed data. Already-expired values must not appear in startup snapshots.

Value expiry, input-claim release, connection liveness, and overdue freshness are separate mechanisms. Instant payloads have no value expiry. Input expiry clears only its value; claim release clears only its ownership. Omitted-expiry behaviour remains a small policy choice in D1.

Schema rules may express a positive fixed expected update interval for retained
state and present desired values; the shortest matching interval applies.
Missing desired values and instant nodes do not become overdue. Becoming
overdue reports retained state under `/$diagnostics/freshness/<source...>`
without deleting the value. Any accepted duplicate write refreshes the deadline
and removes overdue status. This requires timed checks, not only validation
during writes.

Retained data survives restarts through best-effort, coherent, versioned
MessagePack snapshots. Acknowledgement does not promise disk durability. No
event recovery is required, and sessions cannot restart as live connections.

## Schemas

User schemas govern ordinary topics. Their basic interface asks whether a value is valid for its topic, with structured errors. Specific validators can expand later without redesigning the core.

Direct validation may attempt a cast and validate the result again. A successful direct cast stores the resulting value. Failed validation follows the configured rejection/warning policy. The schema chooses warning (log and allow) or error (deny). Only denying constraints guarantee valid values reach consumers. No global strictness policy overrides that choice (D3).

Schema activation and replacement are atomic. A newly installed schema must
not overlap rules in another installed schema: two patterns conflict when some
valid topic could match both. Rules inside one schema may overlap and every
matching validator must pass, but intersecting casting rules are invalid so
conversion never depends on rule ordering. Initial casts are explicit string
to integer, float, or boolean conversions.

A rule may also constrain the node kind. Without an applicable node-kind
constraint, an implicit kind change retains the ordinary warning behavior.
Warning constraints allow with a diagnostic; denying constraints reject the
whole batch.

Normal installation rejects and reports existing deny violations. An explicit
force installation removes deny-invalid state nodes and clears only the
deny-invalid current value of desired inputs, preserving their definitions and
claims. Warning violations remain and are reported. Existing stored data is
never silently cast during installation. Advanced schema language features can
wait; these structural rules cannot be implicit.

The initial management operation is an atomic complete declaration at
`PUT /v1/schemas/{name}`. A future readable projection may also live under
`/$schemas/<name>/...`; that projection is not selected yet.

## Links

Links provide alternative views of the same data, including subtree views. Maintain forward target information for reads and reverse source information for update propagation. Reverse propagation must account for ancestor subtree links, not just exact paths.

If a directly valid source update violates a schema through a link, the link is
in violation. The source write remains valid. A violating descendant affects
the whole containing link, not just that descendant. Direct source propagation
does not cast merely to make a linked view valid.

Links support writes. A write addressed through an alias first applies the
alias schema, including its possible cast, translates the resulting operation
to the canonical topic, then applies the canonical schema and its possible
cast. Unusual multi-cast outcomes are logged. Either denying failure rejects
the entire write. A disabled alias accepts the same process as a repair
attempt; success commits and re-enables it atomically, while failure leaves it
disabled.

A rejecting linked-schema violation from a direct canonical write disables the
link while leaving the canonical write valid. It automatically re-enables once
valid again. An invalid instant event remains visible canonically, is
suppressed through the rejecting link, and disables it. A later valid event
re-enables the link and is delivered without replay. Consumers must not receive
a newly invalid value through a rejecting linked schema. If an exposed view
disappears, removal refers to its last exposed state.

The initial topology rejects cycles, overlapping destination mounts,
destination collisions with existing nodes/subtrees, targets reached through
another link, and links crossing into or out of the system namespace. Link
definitions persist when their canonical target is absent and re-evaluate when
it appears.

## Reserved system namespace

A first path segment starting with `$` is reserved for Tanuki, including its descendants. Examples include `/$connections`, `/$schemas`, and `/$system`.

User schemas do not run on system topics; schemas targeting them are invalid. Built-in validation governs permitted operations and returns distinct errors, such as an attempt to alter another session's properties. A warning schema cannot bypass these checks.

For schema matching, broad selectors such as `/**` range only over ordinary
topics. An explicitly system-rooted schema rule is invalid. Initial links may
not cross the system boundary. System topics can otherwise participate in
observation where exposed.

Central diagnostics use a read-only built-in `/$diagnostics/**` tree visible
through ordinary snapshots and subscriptions. Active overdue freshness and
disabled-link conditions are retained; recovery removes the condition without
replay. Client writes are forbidden. Failure to publish a diagnostic is logged
locally and does not recursively publish another diagnostic. The future runtime
may expose script status separately, but the initial server is diagnosable
without that runtime.

## Transports and delivery order

Start with basic HTTP reads/writes, then quickly build WebSocket as the first complete transport. JSON text and MessagePack binary are intended. WebSocket text messages carry JSON; binary messages carry MessagePack. Codec tags and limits require D5 before freezing the wire contract.

HTTP should support convenient single writes and reads and multi-topic operations. URL parameters may provide shorthands such as `expire=1h`, but adapters must translate to common values and operations. Exact verbs and string conversion rules remain open (D8).

WebSocket reads are subscriptions; unsubscribe is not an initial priority. HTTP SSE, raw TCP, and MQTT can follow later. TCP framing and protocol detection need not delay WebSocket.

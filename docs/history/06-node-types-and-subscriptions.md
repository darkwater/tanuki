# Node types and subscriptions

## Decisions: two independent node flags

The node type consists of two booleans rather than a three-value enumeration. Names below are illustrative: `retained` and `input`.

| | Output: owner writes | Input: other permitted clients write |
| --- | --- | --- |
| Retained | Reported state, e.g. actual TV input | Persistent input, e.g. desired TV input |
| Instant | Occurrence, e.g. button pressed | Submitted occurrence or action request, e.g. toggle power |

Input/output is relative to the owner. Writing an input is not an ownership transfer. Owner writes to its own input might produce a minor warning; exact permissions and warning rules remain open.

Retained input is distinct from retained output. Scripts individually decide how to interpret and act on persistent inputs. There is no agreed core policy for reconciliation, intent expiry, or replaying device actions.

Every accepted write updates the node's last-updated field, including a write whose value equals the previous value. Consumers may use this to assess intent age. Timestamp source/precision, observation timestamps, and transport-redelivery deduplication are not specified. Retention here does not itself settle persistence across server restart or guarantee delivery of instant events.

## Current direction: whole-input computation

The user prefers business logic expressed as an input-to-output function that recalculates its entire output when any relevant input changes. A possible subscription selects a shape and presents the complete shape on updates, instead of making each script assemble unrelated topic messages.

Whether this is implemented in the core or a client library remains open. No query language, callback API, or wire representation is selected.

## Proposals for discussion

Separate three concerns:

1. **Selection:** which nodes and metadata belong to the input shape.
2. **Delivery:** an initial snapshot followed by full snapshots or deltas.
3. **Evaluation:** business logic always receives the current complete input shape.

A client library could apply deltas to a local view and invoke a function with that view. A whole-shape transport could expose the same programming model without local reconstruction.

Some guarantees need core support even if shape assembly lives in the library: an initial snapshot with no gap before live updates; explicit deletions; and atomic write batches (now explicitly required; see below). Revisions and batch markers are possible mechanisms, not selected designs. A coherent snapshot still does not make separate producer writes into one atomic update.

Instant events cannot be reduced to a retained snapshot without losing occurrence information. Consider delivering an occurrence alongside the current retained input shape. Queueing, ordering, loss tolerance, and replay remain open.

The required duplicate-write timestamp update means value equality alone does not imply no change. A possible subscription option distinguishes value changes from metadata changes. This could prevent unnecessary recomputation and feedback loops, but must not hide timestamps from scripts that use freshness. Such filtering is a proposal, not an agreed default.


## Requirements and examples added 2026-10-05

### Subscription selections

User examples:

- Battery widget: `/battery/*`.
- Motion-derived location: `/smart-home/entities/{sensor-kitchen,sensor-desk,sensor-couch}/motion`.
- Dashboard: specific arbitrary topics from different parts of the namespace.

These establish the need for wildcard collections, finite selections, and unions of unrelated paths. Exact syntax and wildcard depth remain to be specified. Shapes may need both maps with dynamic membership and fixed named fields.

Additional assistant examples, not new requirements:

| Consumer | Selection idea | Shape |
| --- | --- | --- |
| TV sidecar | TV power/input, assistant status, estimated location | Named fields from unrelated paths |
| Script-health display | `/runtime/scripts/*/status` | Map keyed by script name |
| Lighting scene editor | Hue and brightness for selected lamps | Map of lamp objects |

### Topic expiry — requirement

Topics should support an expiry time. Old battery entries may only be useful for about a week. Whether expiry removes the value, the complete node, or marks it expired is unresolved. A sliding TTL refreshed by writes, an absolute deadline, and preservation of ownership/schema metadata are design questions.

Proposal: wildcard views should reflect expiry as a membership change when expiry means removal. A renewed write must not be deleted by an obsolete expiry timer.

### Atomic writes and atomic subscription updates — requirement

Support atomic writes to multiple topics. For example, lamp hue and brightness may be separate topics but written together. Subscribed clients must receive the matching topic/value changes as one atomic update and must not expose a partially applied batch to business logic.

Proposal: validate all writes before commit and reject the whole batch if any write fails. A subscriber matching only part of a batch receives that matching subset together. Full snapshots and deltas can both implement these semantics. Exact transaction limits, representation, and mixed retained/instant batches are open.

Protocol design must preserve this guarantee where exposed. Do not silently translate a batch into independent observable updates. The user expects MQTT may lack atomic batches, possibly with an extension using MQTT 5 properties. This is an unverified possibility, not a selected mechanism or guarantee. The native protocol is expected to carry full semantics; MQTT compatibility may expose a limited subset.

### Rust shapes — current direction

The user suggested Serde-based typed shapes, including wrappers that provide extra features:

```rust
#[derive(Deserialize)]
struct TvState {
    on: bool,
    active_input: HdmiInput,
    desired_input: TanukiInput<HdmiInput>,
}
```

User clarification: a plain type such as `i32` exposes only the value. A wrapper adds access to all metadata, the ability to send values subject to permissions, and potentially other facilities. This is a richer view of the same node, not merely an input-write helper. Exact wrapper names and methods remain open. A plain value projection and a live bound handle are separate responsibilities: a library would need node-path and client context to attach write behaviour. Plain deserialization alone should not be assumed to discover subscriptions or produce bound handles.

Possible designs include explicit shape selection followed by typed decoding, a separate Tanuki derive/binding step, or context-aware deserialization. No implementation mechanism is selected. Missing/expired fields and malformed values need explicit handling; an absent value should not silently become false or another ordinary value.


## Native protocol and update envelope — current direction

The main protocol is expected to be custom. Candidate transports are WebSocket and/or TCP; candidate encodings are JSON and/or a binary format such as Postcard. None is selected. Blob support is under consideration, with copying clipboard images across machines as a concrete use case.

The user proposed batched updates containing topic paths, values, metadata, and removal status. On removal, the included value and metadata describe the last state before removal. An enum should prevent consumers accidentally using a removed value as current state.

Assistant refinement, illustrative and not a finalized protocol:

```rust
struct Update {
    topics: Vec<UpdateTopic>,
}

struct UpdateTopic {
    topic: String,
    change: Change,
}

enum Change {
    Present { value: Value, metadata: Metadata },
    Removed { previous_value: Value, previous_metadata: Metadata },
}
```

Instant event representation remains open: an explicit event variant could avoid treating events as retained state. Metadata-only changes and nodes with no previous value also need definition. One update batch is applied fully before exposing it to business logic. Initial snapshot identification, subscription identity, revisions, resynchronization, and event ordering are not specified.

## Remaining subscription decisions — assistant recommendations, not accepted

- Start with an explicit initial snapshot and then ordered atomic batches. Reconnect can discard the local view and obtain a fresh snapshot; retained snapshots cannot recover missed instant events.
- Define missing-field and decoding-error behaviour before exposing strongly typed shapes.
- Define metadata-only notifications and duplicate-write triggers separately from value-change filtering.
- Define slow-consumer policy with bounded queues and explicit lag reporting or disconnection, rather than silent partial batches or event loss.
- Consider separate blob references carrying content type and size, with on-demand byte retrieval, so broad subscriptions do not automatically transfer image payloads. Storage lifetime, permissions, and deletion of referenced blobs remain open.
- Settle selector matching and dynamic membership. Exact encoding and wrapper internals can follow after behavioural choices.


## User clarification: idempotence

The ideal script with only retained/state inputs is idempotent. The user does not consider value-only versus metadata-triggered recomputation an important distinction at present. Do not overdesign wrapper-dependent notification policies. Instant-event consumers remain different. Idempotent external effects do not automatically prevent a self-triggering subscription loop when writes refresh timestamps; loop scheduling can be revisited separately.

See [blob, offer, and stream sketches](07-data-transfers.md) for Rust review sketches and direct transfer proposals. They remain proposals, not implementation decisions.

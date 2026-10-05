# Open questions

Work through these in small chunks. This is a discussion queue, not a request to resolve everything at once.

## Latest operational decisions

See [operational decisions](11-operational-decisions.md): last-writer ownership, client-supplied identity, retained persistence, no event replay, startup subscriptions/snapshot, relative expiry writes, and simple shared endpoint semantics now supersede corresponding questions below. New remaining issue: how last-writer ownership interacts with input/output roles. MessagePack/JSON, small inline bytes, and system namespace decisions also supersede older questions in this historical queue.

## Earlier ownership and connection questions

1. How do owner-only state writes work for periodic phone requests: does ownership belong to a durable identity, with connection liveness tracked separately?
2. How does a caller claim ownership: explicitly, on first write, or by entrypoint policy?
3. What identity appears as last writer when several processes share one machine's credentials?
4. What happens to ownership metadata on disconnect, and how is previous ownership retained?
5. How does a new connection reclaim or take over ownership? What if an older connection is still considered alive?
6. Is one-off interaction style stored explicitly, or represented by an absent owner?
7. Are ownership rules per topic only, or can they apply to a subtree?
8. What liveness timeout and graceful-close semantics are needed?
9. Is each local Unix endpoint a forwarding agent, and how are downstream sessions represented centrally?

## Event direction and permissions

- Settled: node type uses retained/instant and input/output booleans; exact field names and defaults remain open.
- Does everyone mean every admitted client or clients with explicit permission?
- Should owner writes to an incoming event node warn, be silently allowed, or be rejected? The user tentatively suggested a minor warning.
- Who can subscribe to incoming events, and what happens while the owner is disconnected?
- Are command results needed on top of incoming events? A separate command primitive is not decided.

## Later: data model and operations

- What is a topic: an opaque value, JSON document, tree node, or something else?
- How do paths, child nodes, metadata, and document fields relate?
- Which operations on the four node combinations belong in the first version?
- How are schemas attached, versioned, and changed when data already exists?
- How are capabilities and composite entities represented and discovered?
- Which values survive disconnects, expiry, or a core restart?
- How are observation time, acceptance time, freshness, and availability distinguished?
- What consistency and subscription guarantees are actually needed?

## Shape subscriptions — current discussion

- Required selection examples now include wildcards, finite alternatives, and arbitrary path unions. Exact grammar, aliases, and result projection remain open.
- Are shapes native core subscriptions, client-library views, or both?
- Are updates whole snapshots, deltas, or selectable per client?
- How do initial snapshots, subsequent updates, reconnects, additions, and deletions compose without gaps?
- Required: atomic multi-topic writes and atomic delivery of their matching changes. What batch encoding, validation rules, and protocol mappings implement this?
- Which changes trigger recomputation: values, last-updated metadata, ownership, membership?
- Can state updates coalesce? How do instant events retain their occurrence semantics?
- How are missing nodes represented?
- How should scripts avoid feedback loops when duplicate writes refresh timestamps?

## Expiry and typed shapes

- Does expiry remove a value, remove the node, or mark it expired? What happens to ownership and schema metadata?
- Absolute expiry versus sliding TTL, and which writes renew it?
- How do fixed shapes represent missing/expired fields? How are decoding errors surfaced?
- How does a Serde shape map to topic paths and dynamic collections?
- Wrapper scope is clarified: metadata, sending values, and potentially more; plain types expose only values. How is client/path context attached, and how are permissions represented?
- What subset of native semantics can MQTT expose? Atomic MQTT batches are not assumed; extensions remain speculative.

## Update envelopes and binary payloads

- How are initial snapshots distinguished from live changes, and how are updates associated with subscriptions?
- What enum represents present values, removed values with their previous metadata, and instant events?
- Are revisions required initially? On reconnect, rebuild from snapshot or resume?
- What happens when a consumer falls behind? Bound queues and define visible failure/coalescing behaviour.
- Are blobs inline bytes, separately fetched references, or both? What size, lifetime, and access rules apply?
- Native transport and encoding remain choices: WebSocket/TCP and JSON/binary were suggested, not selected.

## Later: operation and implementation

- HTTP authentication and authorization; local socket policy; identity mapping.
- MQTT subset and mapping, including validation errors and connection semantics.
- Script runtime versus external processes; reactive state derivation versus event callbacks.
- Logging, diagnostic retention, and script supervision.
- Storage engine, implementation language, deployment, and restart recovery.
- Scope of the first useful implementation and concrete acceptance scenarios.

## Candidate scenarios to formalize

- A phone posts battery state and disconnects; the value remains without becoming "offline".
- Two laptop processes publish independently; losing one does not invalidate the other's topics.
- A value stays unchanged while its owner's connectivity changes; watchers can learn this.
- A reconnecting publisher replaces an old session; late cleanup does not invalidate the replacement.
- A malformed write under a schema is reported predictably without corrupting valid state.

- An atomic hue/brightness write is observed together, without an intermediate mixed pair.
- A wildcard battery view reflects new entries and entries that expire, according to the chosen expiry semantics.

These are proposed verification scenarios. Their precise expected outcomes depend on the pending decisions above.

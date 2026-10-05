# Topic provenance and ownership

## Latest decision — supersedes ownership proposals below

For output nodes, ownership is the client name of the last accepted writer; changes warn rather than reject by default. Input nodes have a separate define/claim operation for ownership and metadata; submitting an input value never claims ownership. Claiming an input may be optional. See [operational decisions](11-operational-decisions.md). Older ownership proposals below are historical.

## Earlier direction from the user

Attach metadata to topics that can express:

- This topic was last written by `smartphone` through a one-off interaction.
- This topic is currently owned by `laptop`, whose connection remains active.

This should support several independent processes publishing different parts of the same machine's state. A device-wide flag must not be the only representation of availability.

## Proposed metadata model

| Concept | Meaning | Status |
| --- | --- | --- |
| Last writer | Identity responsible for the current value | Current direction; identity granularity open |
| Last updated | Refreshed on every accepted write, including an identical value | Required; clock source and precise timestamp semantics open |
| Owner | Optional session responsible for maintaining the topic | Current direction; exact ownership target open |
| Ownership availability | Active, disconnected, or never claimed | Proposal; representation open |

Illustrative only:

```yaml
/devices/phone/battery:
  value: { percentage: 72 }
  last_writer: smartphone
  owner: null

/devices/laptop/battery:
  value: { percentage: 85 }
  last_writer: laptop/battery-agent
  owner: session-123
  availability: active
```

A separate process can independently own `/devices/laptop/clipboard`. The names above do not require separate authentication credentials per process.

## Proposed disconnect behaviour

- Keep the last known value and its provenance.
- Mark connection-bound ownership as disconnected/unavailable.
- Allow subscribers to observe metadata changes even when the value is unchanged.
- Do not change an unowned topic merely because the request that wrote it ended.

The exact distinction between a current owner and a previous, disconnected owner must be defined. The example fields are not yet a coherent storage schema.

## Unresolved semantics

**Decision: node type has two independent booleans**, retained/instant and input/output. Direction is from the owner's perspective. See [node types and subscriptions](06-node-types-and-subscriptions.md). Output nodes are owner-written; input nodes accept other writers. Owner writes to input nodes might produce a minor warning; this remains tentative. The scope of permitted external writers remains open.

**Current direction: ownership changes should not happen unexpectedly.** The user allows that a warning may be preferable to blocking due to old leftovers. Explicit takeover with a warning is an assistant proposal, not a chosen mechanism.

**Ownership versus session — newly exposed question:** owner-only state writes must still support periodic phone updates without a persistent connection. Durable identity ownership with an optional active session is an assistant proposal to explore. It would separate write authority from live maintenance. The earlier optional session-owner example above is provisional and does not yet resolve this case.

**One-off metadata:** the user mentioned recording the interaction style. The assistant proposed representing the lack of ongoing ownership instead, because even a persistent connection could submit a one-off value. Whether to retain interaction style separately remains open.

**Freshness:** connection liveness alone does not establish that a value is current. Observation time, acceptance time, and validity deadlines may need separate meanings; this has not been specified.

**Granularity:** per-topic ownership is the current discussion. Subtree ownership and overrides were earlier assistant suggestions and are not agreed.

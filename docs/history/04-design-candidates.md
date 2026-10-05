# Other design candidates

These ideas were discussed before focusing on client lifecycle. Except where explicitly labelled otherwise, they are proposals, not accepted requirements.

## Node model — decision

The user selected two booleans: retained/instant and input/output. This supersedes the earlier three-kind sketch. See [node types and subscriptions](06-node-types-and-subscriptions.md).

Retained input and retained output are distinct. Scripts decide how to interpret persistent intent and its age; the core does not automatically reconcile desired and actual state. A separate command node type is not selected. Request/result conventions can be considered separately if needed.

## Schemas and capabilities

**Current direction:** optional schemas govern namespaces and violations become visible errors.

**Proposal:** validate before accepting a write, return the error to the caller, retain the previous valid value, and expose a diagnostic.

**Proposal:** capabilities describe meaningful interfaces, units, operations, and discovery, beyond payload shape alone. A generic battery widget could discover battery capabilities wherever they live.

No schema language, schema attachment mechanism, versioning rules, or capability registry has been selected. The arbitrary-data workflow must remain easy.

## Runtime and diagnostics

**Current direction:** possible embedded script execution; script status exposed under a runtime namespace; logs available for scripts to forward.

**Proposals:** a supervisor owns failure status so a crashed script need not report its own death; logs are events while health is state; preserve recent errors independently of notification delivery.

**Alternative proposal:** begin with external processes and a useful API/CLI, adding an embedded runtime later. This has not been accepted over the user's embedded-runtime idea.

## Other proposed core facilities

- Snapshot plus subscription without an update gap.
- Atomic document writes for related fields.
- Configurable data lifetimes: retained, session-bound, and expiring.
- Distinguish unavailable from deleted.
- Separate identity and entity relationships from path spelling.

These are candidates to revisit individually, not an approved first-release checklist.

## Protocol boundaries

**Requirement:** MQTT and HTTP should eventually be options for easy client integration; others may follow.

**Proposal:** adapters expose common core operations, including validation, rather than establishing separate authorities for data. Exact mappings and departures from standard MQTT behaviour must be designed explicitly.

No separate MQTT broker is required or desired by the current discussion. The precise meaning of protocol compatibility remains open.

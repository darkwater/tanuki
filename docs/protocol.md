# Protocol

Tanuki does not expose a wire protocol yet. This document records the
transport-neutral semantics that future JSON and MessagePack DTOs must preserve.

## Current operation boundary

A nonempty `WriteBatch` is attributed through a `WriteContext` and applied at
an explicit server timestamp. One accepted batch produces one ordered,
sequence-numbered `UpdateBatch` plus zero or more warnings. A rejected batch
has no node, occurrence, or sequence effects.

State publications retain their latest complete runtime value and provenance.
Event publications update publisher metadata and emit an occurrence in the
current batch; occurrence payloads never appear in snapshots. A snapshot is a
selection-filtered map of current nodes at one commit sequence.

Input definitions exist independently of claims and payloads. Desired
submissions retain their latest value and submitter provenance without changing
the claim. Command submissions emit an occurrence and retain no payload.
Claiming requires a current managed session, but definition and submission may
use an attributed stateless context. A batch may atomically define, claim, and
submit on one input topic in operation order.

The current core rejects duplicate operations in the same input lifecycle slot,
same-topic output/removal combinations, and writes to the reserved `$`
namespace. The repeated-target details remain provisional until the external
batch contract is reviewed. No HTTP routes, message envelopes, codec tags,
correlation identifiers, or persistence format are published yet.

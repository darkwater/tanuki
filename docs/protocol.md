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

The current core rejects repeated canonical targets in one batch and writes to
the reserved `$` namespace. The repeated-target rule is provisional until the
external batch contract is reviewed. No HTTP routes, message envelopes, codec
tags, correlation identifiers, or persistence format are published yet.

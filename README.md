# Tanuki design handoff

Consolidated 2026-10-05.

Tanuki is a personal automation fabric for named state, events, desired inputs, and commands, with shared validation and subscription semantics across transports. Smart home is a primary use case; ad hoc scripts should remain easy.

## Current documents

Start with [AGENTS.md](AGENTS.md) for Rust/nightly, TDD, invariants, diagnostics and collaboration requirements. Then read:

1. [Specification](docs/spec.md) — consolidated agreed direction, with explicit references to unresolved choices.
2. [Decisions to review](docs/decisions-to-review.md) — ambiguities found during review, their implementation impact, and proposed defaults for feedback.
3. [Implementation plan](docs/implementation-plan.md) — phases and sequential task cards with dependencies and acceptance criteria.
4. [Design proposal](docs/design.md) — proposed Rust types, module boundaries, data flows and user review checkpoints.
5. [Test plan](docs/test-plan.md) — unit/integration contracts and full-system simulations with producing/consuming mock clients.

Recommendations are not automatically accepted requirements. Before implementation, record which defaults were selected. Resolve decisions at the phase that needs them; do not require a complete advanced schema language or future transport design before starting the core.

## Living use cases

The [user-story index](docs/user-stories/USER-STORIES.md) links one document per
use case. Read relevant stories alongside the technical plan. Maintain them
through user conversations and real deployment, linking acceptance criteria to
tests and recording what is actually in use.

## Historical discussion notes

The numbered files below are preserved for rationale and examples. They contain earlier proposals and superseded questions. Where they conflict, use the current documents above; do not combine every historical suggestion into the implementation scope.

- [Goals](docs/history/01-goals.md)
- [Connections](docs/history/02-connections.md)
- [Topic metadata](docs/history/03-topic-metadata.md)
- [Design candidates](docs/history/04-design-candidates.md)
- [Open questions](docs/history/05-open-questions.md) — historical question list; the current queue is the decisions review.
- [Node types and subscriptions](docs/history/06-node-types-and-subscriptions.md)
- [Data transfers](docs/history/07-data-transfers.md) — transfer offers and peer-to-peer designs are deferred.
- [Values and encoding](docs/history/08-values-and-encoding.md)
- [Links and schemas](docs/history/09-links-and-schemas.md)
- [System and enums](docs/history/10-system-and-enums.md)
- [Operational decisions](docs/history/11-operational-decisions.md)

The design API snippets remain review sketches rather than compiled interfaces.
Task 00 has bootstrapped the executable, lifecycle seam, test, and CI; domain
and protocol implementation still waits on the documented architecture review.

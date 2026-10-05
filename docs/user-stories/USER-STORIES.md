# Tanuki — living user stories

These documents describe why and how the user expects to use Tanuki. They should evolve into an accurate account of actual scripts, devices and workflows, not remain generic backlog slogans.

## Story index

| ID    | Story                                                                   | Origin / scope                                              |
| ---   | ---                                                                     | ---                                                         |
| US-01 | [Battery monitor](STORY-01-battery-monitor.md)                          | User use case                                               |
| US-02 | [Room location from motion](STORY-02-room-location.md)                  | User use case                                               |
| US-03 | [TV state and sidecar display](STORY-03-tv-sidecar.md)                  | User use case                                               |
| US-04 | [Voice-assistant status everywhere](STORY-04-assistant-indicators.md)   | User use case                                               |
| US-05 | [Desktop state and components](STORY-05-desktop-state.md)               | User use case                                               |
| US-06 | [Clipboard exchange, including small images](STORY-06-clipboard.md)     | User use case                                               |
| US-07 | [Lamp desired state and atomic output](STORY-07-lamp-control.md)        | User use case and accepted lifetime example                 |
| US-08 | [Pending intent before a controller starts](STORY-08-pending-intent.md) | Illustrative assistant example, lifetime behaviour accepted |
| US-09 | [Dashboard combining unrelated topics](STORY-09-dashboard.md)           | User use case                                               |
| US-10 | [Two views of the same data](STORY-10-linked-views.md)                  | User use case                                               |
| US-11 | [Logs, alerts and script health](STORY-11-diagnostics.md)               | User use case; embedded runtime portion deferred            |
| US-12 | [Ad hoc local and remote scripts](STORY-12-adhoc-local.md)              | User use case                                               |

## How to maintain these documents

- Before implementing a user-facing behaviour, read the relevant stories and their acceptance criteria. Use them to design mock producers/consumers and end-to-end examples.
- During conversations, update affected stories when the user clarifies intent, chooses a concrete workflow, or reports actual usage. No separate documentation approval ritual is needed for faithfully recording those decisions.
- Distinguish **intended**, **implemented/tested**, and **in actual use**. Record dates, real script/config paths, test names/commands, observed limitations and any manual verification. Do not mark deployment based on a mock-client test.
- Keep stable US identifiers and criterion IDs so code/tests can reference them. Append new criteria with new IDs; do not renumber existing criteria or silently erase unmet requirements. Mark superseded criteria with a reason when behaviour changes.
- Topic names, examples and timing can become concrete as clients are built. Clearly label proposals until agreed; do not turn illustrative paths into a compulsory ontology.
- Preserve user intent when code falls short: record the gap instead of rewriting the story to make failing behaviour look correct. If the user changes the requirement, update the story, test contract and affected specification together.
- Product semantics remain in `../spec.md`; stories describe applications of them. Flag conflicts and reconcile them from the user's decision rather than silently overriding either document.
- Link each implemented criterion to tests or state why it is manual/deferred. One system scenario can cover several stories; avoid duplicating an entire test suite per story.
- Keep a short change note for meaningful decisions, not a transcript of every conversation. Replace placeholders with reality as evidence becomes available.

## Scope and coverage

The stories cover the concrete examples discussed: batteries, inferred room
location, TV/sidecar, assistant indicators, desktop components including
Bluetooth, clipboard bytes, lighting, pending retained intent, dashboards,
links, diagnostics/runtime status, and ad hoc clients.

US-08's heating example is illustrative, not an assertion that a heating
deployment is planned. US-11's embedded runtime and US-12's socket entrypoint
retain their stated delivery scope. These stories guide the core and external
examples; they do not require Tanuki to ship a complete device adapter or
polished UI for every application.

The composite simulated-room scenario in `../test-plan.md` is a proving ground for
several stories. Smaller story-specific scenarios cover uses that do not belong
naturally in that room simulation. Actual hardware, integrations, algorithms
and dashboards can replace mock actors incrementally without changing core
contracts.

# US-13 — Live system inspector

- **Status:** requested future UI; implementation and deployment unverified.
- **Origin:** User request, 2026-10-31; mqttui interaction reference.
- **Last updated:** 2026-10-31.
- **Lifecycle:** maintained under [USER-STORIES.md](USER-STORIES.md).

## User story

I want a UI similar to mqttui that eventually shows the entire public state of
the server, including node types and the schemas that apply to nodes, so I can
explore and understand the running system.

## Observable acceptance criteria

- **US-13.A1:** Browse public topics and inspect node kinds and values,
  distinguishing absent payloads from Null and retained values from live
  event/command occurrences.
- **US-13.A2:** Start with a complete snapshot and apply whole atomic updates;
  show newly appearing nodes, metadata changes, expiry and removals.
- **US-13.A3:** Inspect applicable schemas and rules for a selected node,
  including enforcement, validation, casts and freshness policy. Distinguish
  no applicable rules from unavailable or stale policy information.
- **US-13.A4:** Eventually cover the entire public server state. Define the
  precise inspection surface before claiming complete coverage.

## Proposed delivery and open choices

A read-only live tree with search/filtering and a node detail pane is a
proposed first increment, showing values, metadata, claims, expiry and exposed
diagnostics. A terminal UI using the native SDK is an interpretation of the
[mqttui reference](https://github.com/EdJoPaTo/mqttui), not a confirmed format
or toolkit. Implementation timing remains open; editing is not yet requested.

Follow with public schema/link enumeration and node policy explanations,
including alias versus canonical policy and definitions matching no current
nodes. Review how policy changes are observed and correlated with data before
implementing that API. Do not duplicate server validation in the UI or present
stale policy as current.

Clarify whether “public” means ordinary topics or all externally inspectable
state, including reserved system branches and configuration. This story adds
no authentication, permissions framework, private-runtime exposure or event
replay requirement. It preserves the existing specification's semantics.

## Implementation and evidence

Existing snapshots/subscriptions expose node kinds and metadata. Schema
installation and link installation/removal exist, but public enumeration and
per-node policy explanation APIs are missing. Existing dashboard tests prove
transport prerequisites, not UI acceptance. All US-13 criteria remain
unverified for an inspector.

Related stories: [US-09](STORY-09-dashboard.md) for composed dashboards and
[US-11](STORY-11-diagnostics.md) for operational diagnostics.

## Change notes

- 2026-10-31: Recorded requested direction, proposed first scope and API gaps.

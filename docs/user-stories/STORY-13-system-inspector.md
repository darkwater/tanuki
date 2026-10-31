# US-13 — Live system inspector

- **Status:** terminal scope confirmed; model and live connection owner tested; terminal rendering and deployment unverified.
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

Checkpoint A was confirmed on 2026-10-31: a read-only native Rust terminal
application named `tanuki-ui`, using the native SDK, with a live topic tree,
search, detail pane and connection/help status. This follows the
[mqttui reference](https://github.com/EdJoPaTo/mqttui). Ratatui/Crossterm is the
candidate toolkit, pending API/version evaluation. The first milestone shows
values, metadata, claims, expiry and exposed diagnostics; schema information
must be labeled unavailable. Editing is outside scope.

Follow with public schema/link enumeration and node policy explanations,
including alias versus canonical policy and definitions matching no current
nodes. Review how policy changes are observed and correlated with data before
implementing that API. Do not duplicate server validation in the UI or present
stale policy as current.

Confirmed public scope includes ordinary topics, protocol-exposed reserved
system branches and eventual public configuration inspection. It excludes
private process memory. The exact complete-state inventory remains to be
verified before claiming A4. This story adds no authentication, permissions
framework, private-runtime exposure or event
replay requirement. It preserves the existing specification's semantics.

## Implementation and evidence

Existing snapshots/subscriptions expose node kinds and metadata. Schema
installation and link installation/removal exist, but public enumeration and
per-node policy explanation APIs are missing. Existing dashboard tests prove
transport prerequisites, not UI acceptance. All US-13 criteria remain
unverified end to end for an inspector.

Task 01 evidence: `tanuki-ui/tests/inspector.rs` contains 11 headless tests
covering empty state, real parent and child nodes, all four kinds, absent versus
Null payloads, atomic batches/removals, search/collapse/selection, metadata and
sequence gaps, malformed paths, terminal controls, bytes/time and untruncated
details. This is partial A1/A2 model evidence, not a live inspector or deployment.
A3/A4 remain unmet. Provisional defaults are collapsed branches, case-sensitive
path filtering and nearest surviving selection. No occurrence history is stored;
instant details explicitly say there is no retained payload. Full text is
available to a future scrolling pane; scrolling itself is Task 03.

Task 02 evidence: `tanuki-ui/tests/live.rs` starts the production server on
loopback and verifies initially empty/populated baselines, newly matching paths,
atomic lamp updates, metadata-only refresh and scheduled expiry, diagnostics in
`/**` and explicit `/$diagnostics/**`, duplicate-name replacement, stale display,
failed reconnect and removal of absent cached nodes on successful reconnect.
Generated local client names coexist. Cancelled idle receives and delayed draws
do not skip queued batches. App unit tests establish a write between hello and
registration and verify that the newer baseline is used; inject terminal lag
without timing races; and cover typed failure/currentness and joined close.
These extend A1/A2 evidence at the model/transport boundary. Rendering/controls
remain Task 03, and A3/A4 remain unmet. No deployment is claimed.

Verified public-surface limitation: the current server exposes active
`/$diagnostics/**`, but has no `/$connections/**` node projection. Thus the
observer participates in managed sessions and duplicate-name replacement, but
cannot yet display itself as a system node. This preserves the eventual public
state goal and records the gap without expanding Task 02 into server work.

Related stories: [US-09](STORY-09-dashboard.md) for composed dashboards and
[US-11](STORY-11-diagnostics.md) for operational diagnostics.

## Change notes

- 2026-10-31: Recorded requested direction, proposed first scope and API gaps.

- 2026-10-31: User confirmed checkpoint A; Task 01 delivered the headless
  model/formatting foundation. Live connections and terminal rendering remain
  pending.

- 2026-10-31: Task 02 added tested SDK ownership/lifecycle and real loopback
  evidence; recorded missing connection-topic projection and pending rendering.

Implementation planning: [bounded inspector handoff](../inspector-implementation-plan.md).

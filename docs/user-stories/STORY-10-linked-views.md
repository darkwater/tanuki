# US-10 — Two views of the same data

- **Status:** implemented and integration-tested; real-world use not yet verified.
- **Origin:** User use case.
- **Last updated:** 2026-10-05.
- **Lifecycle:** maintained under [USER-STORIES.md](USER-STORIES.md).

## User story

I want convenient alternative topic layouts without copying values or maintaining duplicate publishers.

## Situation and flow

Links provide another view of a node or subtree. A dashboard-specific layout can refer to canonical device data. Linked schemas can impose stricter view requirements.

Illustrative canonical `/devices/lamp/...` ↔ linked `/views/room/lamp/...` → readers and writers. These paths are examples, not a prescribed device ontology.

Topic paths, payload layouts and timing examples are illustrative unless explicitly agreed elsewhere. This is an application story, not a mandatory global topic convention.

## Observable acceptance criteria

- **US-10.A1:** A reader of the alias receives current target data and subsequent updates.
- **US-10.A2:** A writer can use an enabled alias under the agreed canonical/alias validation rules.
- **US-10.A3:** If a valid canonical update violates a denying linked schema, the source commits and the entire offending link disables.
- **US-10.A4:** Subscribers lose the invalid view without receiving its invalid candidate value; last-visible state is used for removals.
- **US-10.A5:** Repairing the data or applicable policy automatically re-enables the link and logs recovery.

## Details to learn through use

Actual useful layouts remain to be learned through use. Alias-first validation,
disabled-link repair, and event recovery now follow the accepted D4 profile;
link chains are deliberately deferred.

## Implementation and evidence

- Core test links: [test plan](../test-plan.md) — K1; E1 step 6.
- Story-specific evidence: `tanuki/tests/core_links.rs` covers projection, alias writes,
  invalidation/removal, repair, policy recovery, instant events, topology, and
  replacement/removal. `tanuki/tests/http_api.rs`, `tanuki/tests/websocket_api.rs`, and
  `tanuki/tests/persistence.rs` cover adapters and restart.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.
- 2026-10-06: Implemented the accepted writable-link profile and mapped automated evidence; deployment verification remains open.

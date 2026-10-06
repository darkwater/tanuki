# US-09 — Dashboard combining unrelated topics

- **Status:** simulated live WebSocket and SSE dashboard delivery implemented; rendering and real-world use not yet verified.
- **Origin:** User use case.
- **Last updated:** 2026-10-06.
- **Lifecycle:** maintained under [USER-STORIES.md](USER-STORIES.md).

## User story

I want a dashboard to assemble whatever state is useful, even when its inputs live in unrelated parts of the tree.

## Situation and flow

A dashboard combines batteries, TV, assistant status and desktop workspace. Its business logic treats the selected state as one input and recomputes a view after updates.

Union of arbitrary topic selectors → initial snapshot plus atomic updates → locally maintained input shape → rendered dashboard. A serde metadata/input wrapper is a later ergonomic option.

Topic paths, payload layouts and timing examples are illustrative unless explicitly agreed elsewhere. This is an application story, not a mandatory global topic convention.

## Observable acceptance criteria

- **US-09.A1:** Startup yields a complete selected snapshot, including an explicitly empty result if nothing exists.
- **US-09.A2:** Updates are applied as whole batches before rendering a new view.
- **US-09.A3:** A topic matched through overlapping selectors is not duplicated.
- **US-09.A4:** Removed nodes, missing payloads and actual Null values remain distinguishable.
- **US-09.A5:** A newly relevant topic matching a wildcard appears without restarting the dashboard.

## Details to learn through use

Actual UI, selections and struct/shape mapping; whether any client needs server-supplied full snapshots on each update.

## Implementation and evidence

- Core test links: [test plan](../test-plan.md) — C2, C3, T1; E1 dashboard; add wildcard membership fixture.
- `tests/core_subscriptions.rs` verifies US-09.A1, A3, and A5 at the core boundary, including explicit empty snapshots and newly matching wildcard topics.
- `tests/client_view.rs::complete_update_is_applied_before_the_new_shape_is_observed` verifies US-09.A2 in the minimal client projection.
- `tests/client_view.rs::removals_change_shape_and_occurrences_stay_out_of_retained_nodes`, `tests/protocol_subscription.rs::desired_missing_and_submitted_null_have_distinct_node_views`, and the domain value tests cover the transport-neutral portion of US-09.A4.
- `tests/websocket_api.rs::simulated_room_actors_drive_downstream_outputs_across_transports` verifies a dashboard selection receiving battery, desired lamp, paired actual lamp, and derived location updates over a real loopback WebSocket.
- `tests/http_api.rs::sse_starts_with_a_snapshot_then_preserves_an_atomic_update_batch`
  verifies snapshot-first SSE delivery, whole-batch updates, and fresh-snapshot
  reconnect behavior through the production router.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.
- 2026-10-06: Task 05 implemented coherent selector-union snapshots, atomic delta application, and local selected shape maintenance. WebSocket delivery and an actual dashboard remain pending.
- 2026-10-06: Task 06 exercised the dashboard wire path with simulated actors. No UI rendering or deployed dashboard has been verified.
- 2026-10-06: The read-only SSE adapter added a low-ceremony dashboard stream
  with the same snapshot/atomic-update boundary and no replay promise.

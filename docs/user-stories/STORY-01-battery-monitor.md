# US-01 — Battery monitor

- **Status:** simulated HTTP/WebSocket publication, live delivery, and expiry implemented; real-device use not yet verified.
- **Origin:** User use case.
- **Last updated:** 2026-10-06.
- **Lifecycle:** maintained under [USER-STORIES.md](USER-STORIES.md).

## User story

I want a widget showing battery levels across my devices, regardless of how each device can publish updates.

## Situation and flow

A laptop script maintains a connection; a phone automation posts periodic one-off updates. Widgets read the same retained state. Neither a mandatory device registry nor one producer per physical device is required.

Laptop publisher + phone HTTP task → retained battery topics → one or more widgets. Example selection: `/battery/*`. A percentage-only value is a useful first fixture; richer records remain possible and do not create child topics.

Topic paths, payload layouts and timing examples are illustrative unless explicitly agreed elsewhere. This is an application story, not a mandatory global topic convention.

## Observable acceptance criteria

- **US-01.A1:** A newly opened widget receives all currently matching retained values, then sees updates without restarting.
- **US-01.A2:** A phone post is attributed but creates no managed session and does not kick a same-name connected publisher.
- **US-01.A3:** Duplicate values refresh last_updated. The widget can distinguish stale data from a managed publisher disconnect; a completed phone request does not mean the phone is offline.
- **US-01.A4:** Configured expiry eventually removes obsolete battery entries; the earlier suggestion of about a week is illustrative, not a fixed default.
- **US-01.A5:** When configured, a warning schema allows an unusual value with diagnostics, while a denying schema blocks it.

## Details to learn through use

Percentage representation, charging state, publication intervals, freshness threshold, actual widget platform and final topic names.

## Implementation and evidence

- Core test links: [test plan](../test-plan.md) — C1, C3, L2, S1, T1; E1 steps 1 and 4.
- `tests/core_publication.rs::duplicate_state_write_refreshes_provenance_and_expiry` verifies the core portion of US-01.A3. Connection distinction and widget observation remain tasks 03–06.
- `tests/core_publication.rs::snapshot_selection_filters_topics_and_removal_carries_previous_state` verifies selected one-off retained reads; transport-neutral live startup/update handoff is covered by task 05 tests and network delivery remains task 06.
- `tests/http_api.rs::battery_publish_and_read_work_over_a_real_tcp_listener` verifies a stateless phone-style HTTP publication and anonymous selected read through the production router. It is simulated local traffic, not a deployed phone integration.
- `tests/core_subscriptions.rs::snapshot_then_newer_update_has_no_registration_gap` verifies the core snapshot-plus-live-update portion of US-01.A1.
- `tests/websocket_api.rs::simulated_room_actors_drive_downstream_outputs_across_transports` verifies laptop WebSocket plus phone HTTP publication reaching a live dashboard. `same_name_http_is_stateless_but_duplicate_websocket_replaces_session` covers US-01.A2 and managed replacement on real loopback sockets.
- `tests/websocket_api.rs::explicit_expiry_and_disconnect_grace_flow_through_live_transports` verifies US-01.A4 with controlled wall/monotonic time and an observed removal batch. Refresh/stale-timer guards are covered by `tests/core_expiry.rs` and `tests/scheduler.rs`.
- `tests/persistence_server.rs` verifies a retained battery value survives an orderly save and production restart path; actual deployment persistence remains unverified.
- `tests/http_api.rs::schema_installation_casts_valid_http_writes_and_denies_invalid_ones`
  verifies US-01.A5's denying path and an explicit string-to-integer cast.
  `tests/core_schema.rs::warning_rules_accept_the_original_value_and_return_a_diagnostic`
  covers its warning path.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.
- 2026-10-05: Task 02 implemented retained output refresh and selection-filtered core snapshots; no transport or deployment claim yet.
- 2026-10-05: Task 04 added the first real HTTP battery publish/read path; live subscription and actual-device verification remain pending.
- 2026-10-06: Task 05 added coherent selected core snapshots and newer update delivery; no WebSocket or actual widget has been verified yet.
- 2026-10-06: Task 06 added real loopback JSON/MessagePack WebSockets and simulated laptop/phone-to-dashboard delivery. No deployed widget or device has been verified.
- 2026-10-06: Task 07 added deterministic retained-value expiry and live removal delivery; expiry durations for actual devices remain unconfigured.
- 2026-10-06: Task 08 added best-effort retained-state restart coverage.
- 2026-10-06: Task 09 added atomic battery schema installation, warning/deny
  enforcement, explicit casts, and schema persistence; no real device schema
  has been deployed.

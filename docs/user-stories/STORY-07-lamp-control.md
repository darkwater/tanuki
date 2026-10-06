# US-07 — Lamp desired state and atomic output

- **Status:** simulated input ownership, atomic output, value expiry, and claim grace implemented; real lamp use not yet verified.
- **Origin:** User use case and accepted lifetime example.
- **Last updated:** 2026-10-07.
- **Lifecycle:** maintained under [USER-STORIES.md](USER-STORIES.md).

## User story

I want scripts and controls to request lighting changes while widgets see a coherent report of the lamp’s actual state.

## Situation and flow

A controller claims inputs. Other clients submit desired brightness or related settings without taking ownership. The controller publishes actual values, possibly on separate topics.

Controls/automation → illustrative `/lamp/desired-brightness` → controller → `/lamp/hue` + `/lamp/brightness` in an atomic batch → dashboard.

Topic paths, payload layouts and timing examples are illustrative unless explicitly agreed elsewhere. This is an application story, not a mandatory global topic convention.

## Observable acceptance criteria

- **US-07.A1:** Input submission updates value provenance without changing the controller claim.
- **US-07.A2:** Hue and brightness from one batch become visible together.
- **US-07.A3:** Desired value expiry clears the request, preserving input definition and claim; it does not automatically undo physical effects.
- **US-07.A4:** During a configured disconnect grace, requests continue to be stored. Reclaiming preserves the latest unexpired request.
- **US-07.A5:** Claim release leaves the input unclaimed and does not erase its retained value.

## Details to learn through use

Actual lamp adapter, colour representation, competing automation/manual-control policy and expiry durations. A brightness of 70 and ten-minute expiry are fixtures only.

The [client API proposal](../client-api-design.md) uses this story to exercise
explicit claim/submission methods and typed atomic writes. Native SDK coverage was added on 2026-10-07; this remains simulated
client evidence, without a verified lamp adapter.
WebSocket observation helpers will expose complete latest snapshots of paired
state; clients needing every command or transition use raw update listeners.

## Implementation and evidence

- Native SDK evidence: `tests/native_sdk.rs::typed_lamp_handles_keep_claims_explicit_and_batch_rejection_atomic` verifies explicit ownership, remote desired submission, paired output and rejected mixed-batch rollback; `sdk_observers_receive_timer_removal_and_desired_expiry_without_losing_claim` covers desired expiry with the claim intact. The compiled controller accepts `/lamp/desired-brightness` and simulates an atomic `/lamp/hue` + `/lamp/brightness` report; a dashboard observed that report on loopback. No physical lamp action is verified.

- Core test links: [test plan](../test-plan.md) — C1, C2, L1, L2; E1 steps 2 and 5.
- `tests/core_publication.rs::two_state_writes_form_one_coherent_commit_batch` verifies the core commit portion of US-07.A2. `tests/client_view.rs::complete_update_is_applied_before_the_new_shape_is_observed` verifies transport-neutral whole-batch consumption.
- `tests/core_sessions_inputs.rs::claiming_preserves_value_and_submission_preserves_claim` verifies US-07.A1, and `disconnect_immediately_releases_claim_but_preserves_desired_value` verifies the immediate-release portion of US-07.A5.
- `tests/websocket_api.rs::simulated_room_actors_drive_downstream_outputs_across_transports` verifies a remote submission reaches the controller and its paired hue/brightness update reaches the dashboard as one WebSocket batch. `disconnect_immediately_releases_an_owned_input_claim` covers socket disconnect cleanup.
- `tests/core_expiry.rs` and `tests/scheduler.rs` verify US-07.A3–A5, including stale deadline/claim guards. `tests/websocket_api.rs::explicit_expiry_and_disconnect_grace_flow_through_live_transports` observes submission during disconnect grace and later claim release while retaining the desired value.
- `tests/persistence.rs::coherent_save_restore_filters_expired_values_and_clears_live_authority` verifies desired definition/value restoration with the old claim cleared.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.
- 2026-10-05: Task 02 implemented the atomic hue/brightness core commit boundary; consumer delivery and actual lamp integration remain unverified.
- 2026-10-05: Task 03 implemented distinct claim and submitter identity, claim-preserving submissions, and immediate claim release. Timer-driven criteria remain pending.
- 2026-10-06: Task 06 verified the desired-input and atomic actual-output chain with simulated actors over real loopback transports.
- 2026-10-06: Task 07 implemented and simulated desired-value expiry and guarded disconnect grace. Physical lamp behaviour remains outside Tanuki and unverified.
- 2026-10-06: Task 08 added restart persistence for desired definitions and unexpired values, with claims deliberately cleared.

- 2026-10-07: Native SDK delivery added typed WebSocket clients and raw/latest-state observation tests, plus compiled loopback examples. Browser bindings and Iced integration remain deferred; no real hardware/deployment verification was added.

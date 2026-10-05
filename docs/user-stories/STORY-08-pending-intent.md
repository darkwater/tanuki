# US-08 — Pending intent before a controller starts

- **Status:** pending-intent core lifecycle implemented; expiry, persistence, transport, and real-world use not yet verified.
- **Origin:** Illustrative assistant example, lifetime behaviour accepted.
- **Last updated:** 2026-10-05.
- **Lifecycle:** maintained under [USER-STORIES.md](USER-STORIES.md).

## User story

I want a retained request to wait for a controller that is temporarily absent, rather than disappear because the handler started later.

## Situation and flow

The discussion used a desired heating temperature of 22 with a one-hour expiry. This is a lifecycle example, not a confirmed real heating integration.

Phone → illustrative unclaimed `/heating/desired-temperature` → later controller claim/subscription → application-specific action. Creation specifies retained input kind.

Topic paths, payload layouts and timing examples are illustrative unless explicitly agreed elsewhere. This is an application story, not a mandatory global topic convention.

## Observable acceptance criteria

- **US-08.A1:** An attributed stateless client can submit to an unclaimed retained input without acquiring a session claim.
- **US-08.A2:** A later controller receives the pending value and claiming does not erase it.
- **US-08.A3:** If the value expires first, the later controller sees no current request, not an implicit Null or zero.
- **US-08.A4:** After a completed best-effort save and restart, the definition and unexpired value return; the old session claim does not.

## Details to learn through use

Whether this is used for heating at all, actual units, controller behaviour and request lifetime. Keep the generic lifecycle even if this example is replaced by a real use case.

## Implementation and evidence

- Core test links: [test plan](../test-plan.md) — L1, P1; E1 restart coverage.
- `tests/core_sessions_inputs.rs::stateless_definition_and_submission_create_unclaimed_pending_intent` verifies US-08.A1.
- `tests/core_sessions_inputs.rs::claiming_preserves_value_and_submission_preserves_claim` verifies the claim-preservation part of US-08.A2. Network subscription delivery remains task 06; expiry and restart criteria remain tasks 07 and 08.
- `tests/http_api.rs::batch_endpoint_composes_unclaimed_input_definition_and_submission` verifies US-08.A1 through the stateless HTTP batch adapter.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.
- 2026-10-05: Task 03 implemented stateless pending-intent creation and later claim without value loss; delivery, expiry, and persistence remain unverified.

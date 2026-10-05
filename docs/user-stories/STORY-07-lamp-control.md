# US-07 — Lamp desired state and atomic output

- **Status:** atomic output core behavior implemented; input lifecycle, transport, and real-world use not yet verified.
- **Origin:** User use case and accepted lifetime example.
- **Last updated:** 2026-10-05.
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

## Implementation and evidence

- Core test links: [test plan](../test-plan.md) — C1, C2, L1, L2; E1 steps 2 and 5.
- `tests/core_publication.rs::two_state_writes_form_one_coherent_commit_batch` verifies the core commit portion of US-07.A2. Subscription delivery remains task 05; input criteria remain tasks 03 and 07.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.
- 2026-10-05: Task 02 implemented the atomic hue/brightness core commit boundary; consumer delivery and actual lamp integration remain unverified.

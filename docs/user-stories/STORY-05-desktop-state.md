# US-05 — Desktop state and components

- **Status:** intended use; implementation and real-world use not yet verified.
- **Origin:** User use case.
- **Last updated:** 2026-10-05.
- **Lifecycle:** maintained under [USER-STORIES.md](USER-STORIES.md).

## User story

I want scripts and interfaces to consume desktop state such as open applications, current workspace and Bluetooth state.

## Situation and flow

Several independent processes on one Linux system may publish different parts of its state. This must not require a single all-encompassing device connection.

Workspace/application/Bluetooth observers → illustrative `/desktop/workspace`, `/desktop/applications`, `/desktop/bluetooth/...` → scripts and dashboards. Topic hierarchy does not imply connection identity.

Topic paths, payload layouts and timing examples are illustrative unless explicitly agreed elsewhere. This is an application story, not a mandatory global topic convention.

## Observable acceptance criteria

- **US-05.A1:** Independent desktop producers coexist without ownership being assigned to an entire physical device.
- **US-05.A2:** Consumers can select only the paths they need or combine them with unrelated state.
- **US-05.A3:** A map describing applications remains a single topic value unless the producer explicitly writes separate topics.
- **US-05.A4:** One producer disconnecting does not mark unrelated desktop producers disconnected.

## Details to learn through use

Actual compositor APIs, Bluetooth integration, per-machine naming and whether applications are one collection or separate explicitly published topics.

## Implementation and evidence

- Core test links: [test plan](../test-plan.md) — U1, C1, C3; add independent-producer system fixture.
- Story-specific test file/command: not yet implemented. Map each criterion to a test or explicitly mark it manual/deferred.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.

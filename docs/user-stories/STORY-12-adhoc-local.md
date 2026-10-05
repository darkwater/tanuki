# US-12 — Ad hoc local and remote scripts

- **Status:** intended use; implementation and real-world use not yet verified.
- **Origin:** User use case.
- **Last updated:** 2026-10-05.
- **Lifecycle:** maintained under [USER-STORIES.md](USER-STORIES.md).

## User story

I want to publish and read useful values from small scripts with minimal ceremony, on both Linux and less flexible portable devices.

## Situation and flow

Local processes should eventually use a permitted Unix socket without additional authentication. A phone may only have a convenient HTTP automation action. Both use the same core meanings.

Local process or HTTP automation → arbitrary named value → another script. A producer can start freeform and add a schema later; registration of physical devices is unnecessary.

Topic paths, payload layouts and timing examples are illustrative unless explicitly agreed elsewhere. This is an application story, not a mandatory global topic convention.

## Observable acceptance criteria

- **US-12.A1:** A one-off HTTP write needs attribution but not a managed session.
- **US-12.A2:** An anonymous read can fetch values without registering a connection.
- **US-12.A3:** A freeform topic accepts a changed runtime kind; unusual changes can warn rather than requiring a migration ceremony.
- **US-12.A4:** A denying schema, once applicable, consistently blocks violations through every supported entrypoint.
- **US-12.A5:** When the Unix-socket entrypoint is delivered, access to that endpoint follows the chosen socket permissions without an additional login step.

## Details to learn through use

Actual shell/phone request examples, Unix-socket transport exposure and deployment access. This story does not silently add the socket adapter to the first HTTP milestone.

## Implementation and evidence

- Core test links: [test plan](../test-plan.md) — C1, S1, T1; HTTP task 04; add socket parity tests when that adapter lands.
- Story-specific test file/command: not yet implemented. Map each criterion to a test or explicitly mark it manual/deferred.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.

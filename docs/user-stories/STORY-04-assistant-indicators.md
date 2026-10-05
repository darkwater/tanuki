# US-04 — Voice-assistant status everywhere

- **Status:** intended use; implementation and real-world use not yet verified.
- **Origin:** User use case.
- **Last updated:** 2026-10-05.
- **Lifecycle:** maintained under [USER-STORIES.md](USER-STORIES.md).

## User story

I want to see when my custom home assistant is listening, on whichever display or light is useful at that moment.

## Situation and flow

The assistant publishes status. Independent consumers render an indicator on the TV, sidecar tablet or ceiling lamps. Tanuki communicates status; it does not need to implement speech recognition.

Assistant → illustrative `/voice-assistant/status` → TV/tablet indicators and lamp adapter. Retained output suits current status; concrete status names remain application-defined.

Topic paths, payload layouts and timing examples are illustrative unless explicitly agreed elsewhere. This is an application story, not a mandatory global topic convention.

## Observable acceptance criteria

- **US-04.A1:** Several consumers see the same published status without the assistant knowing their identities.
- **US-04.A2:** A newly connected indicator receives current status immediately.
- **US-04.A3:** A status change reaches all matching connected consumers; each chooses its own visual representation.
- **US-04.A4:** If the publisher disappears, consumers can use available provenance/freshness metadata rather than assume retained listening status proves it is still listening.

## Details to learn through use

Status enum, expiry/freshness policy, indicator colours and priority against normal lamp control. These need a practical choice when wiring real clients.

## Implementation and evidence

- Core test links: [test plan](../test-plan.md) — C3, T1; add assistant producer with two indicator consumers to the system harness.
- Story-specific test file/command: not yet implemented. Map each criterion to a test or explicitly mark it manual/deferred.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.

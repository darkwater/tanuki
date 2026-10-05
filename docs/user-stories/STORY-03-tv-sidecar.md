# US-03 — TV state and sidecar display

- **Status:** intended use; implementation and real-world use not yet verified.
- **Origin:** User use case.
- **Last updated:** 2026-10-05.
- **Lifecycle:** maintained under [USER-STORIES.md](USER-STORIES.md).

## User story

I want the OLED tablet below my TV to show a useful sidecar view based on TV power, HDMI source and other current context.

## Situation and flow

A TV integration publishes actual state; the sidecar combines it with other selected topics. A control surface may request a different source or send a toggle command; these are distinct from observed TV state.

TV adapter → illustrative `/tv/on` and `/tv/active-input` → tablet. Optional retained input `/tv/desired-input` and instant input `/tv/toggle` → TV adapter → observed outputs.

Topic paths, payload layouts and timing examples are illustrative unless explicitly agreed elsewhere. This is an application story, not a mandatory global topic convention.

## Observable acceptance criteria

- **US-03.A1:** The tablet starts from current state and reacts to source/power changes.
- **US-03.A2:** Related TV fields published in one batch produce one coherent display recalculation.
- **US-03.A3:** Submitting a desired HDMI input does not pretend the observed active input has changed; the adapter reports actual state separately.
- **US-03.A4:** If a toggle command is used, acceptance means publication, not proof the TV acted; commands are not replayed on reconnect.

## Details to learn through use

TV interface, supported HDMI identifiers, tablet app and display/sleep behaviour. Desired-input and toggle endpoints are illustrative options, not obligations to implement both.

## Implementation and evidence

- Core test links: [test plan](../test-plan.md) — C2, C3, L1, T1; E1 TV observer; add sidecar projection fixture.
- Story-specific test file/command: not yet implemented. Map each criterion to a test or explicitly mark it manual/deferred.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.

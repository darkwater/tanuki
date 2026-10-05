# US-01 — Battery monitor

- **Status:** intended use; implementation and real-world use not yet verified.
- **Origin:** User use case.
- **Last updated:** 2026-10-05.
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
- Story-specific test file/command: not yet implemented. Map each criterion to a test or explicitly mark it manual/deferred.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.

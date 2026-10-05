# US-11 — Logs, alerts and script health

- **Status:** intended use; implementation and real-world use not yet verified.
- **Origin:** User use case; embedded runtime portion deferred.
- **Last updated:** 2026-10-05.
- **Lifecycle:** maintained under [USER-STORIES.md](USER-STORIES.md).

## User story

I want unusual behaviour and script failures to be visible centrally, so I can inspect them or have a script notify me.

## Situation and flow

Tanuki logs unexpected situations. A notification script could consume log events and send Telegram messages. A future runtime could own per-script status topics, allowing a UI to list failed scripts.

Core/client diagnostics → logging interface → viewer or optional notifier. Future runtime → illustrative `/$runtime/scripts/...` status → health UI. Exact paths are not selected; earlier `/log/info` was an interface idea.

Topic paths, payload layouts and timing examples are illustrative unless explicitly agreed elsewhere. This is an application story, not a mandatory global topic convention.

## Observable acceptance criteria

- **US-11.A1:** Ownership anomalies, schema warnings, link disable/recovery and operational failures have useful diagnostic context.
- **US-11.A2:** Warnings remain observable even when the associated operation succeeds.
- **US-11.A3:** A diagnostic consumer failure must not recursively generate an unbounded feedback loop.
- **US-11.A4:** Local logging works without an embedded runtime or notification script.
- **US-11.A5:** When runtime supervision is implemented, script failure can be represented independently of whether a notification was sent.

## Details to learn through use

Log schema, event versus retained status, notifier identity/routing and future script runtime. A notifier failing silently is an accepted residual risk; no guaranteed alert delivery service is required.

## Implementation and evidence

- Core test links: [test plan](../test-plan.md) — S1, K1, P1; task 11 diagnostics tests; future runtime tests only when in scope.
- Story-specific test file/command: not yet implemented. Map each criterion to a test or explicitly mark it manual/deferred.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.

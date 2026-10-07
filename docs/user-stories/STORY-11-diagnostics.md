# US-11 — Logs, alerts and script health

- **Status:** active freshness/link conditions implemented; broader operational diagnostics and real-world use not yet verified.
- **Origin:** User use case; embedded runtime portion deferred.
- **Last updated:** 2026-10-07.
- **Lifecycle:** maintained under [USER-STORIES.md](USER-STORIES.md).

## User story

I want unusual behaviour and script failures to be visible centrally, so I can inspect them or have a script notify me.

## Situation and flow

Tanuki logs unexpected situations. A notification script could consume log events and send Telegram messages. A future runtime could own per-script status topics, allowing a UI to list failed scripts.

Core/client diagnostics → logging interface → viewer or optional notifier.
Future runtime → illustrative `/$runtime/scripts/...` status → health UI. Active
conditions currently use `/$diagnostics/freshness/**` and
`/$diagnostics/links/*`; transient warnings remain in structured outcomes and
local logging.

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

- Automated evidence: `tanuki/tests/core_freshness.rs` covers overdue/recovery state,
  exclusions, and recursive-failure prevention; `tanuki/tests/scheduler.rs` covers the
  timed wake; `tanuki/tests/core_links.rs` covers link condition onset/recovery; and
  `tanuki/tests/persistence.rs` covers restart derivation.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.
- 2026-10-06: Added read-only active freshness and link conditions under the reserved diagnostics tree.

- 2026-10-07: Disabled-link diagnostic names are validated before installation, avoiding a core-lock panic from `.`/`..`. Restore now rejects canonical reserved nodes and logs accepted schema warnings; US-11.A1/A4 remain supported without a script runtime.

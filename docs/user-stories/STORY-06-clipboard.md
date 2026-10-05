# US-06 — Clipboard exchange, including small images

- **Status:** intended use; implementation and real-world use not yet verified.
- **Origin:** User use case.
- **Last updated:** 2026-10-05.
- **Lifecycle:** maintained under [USER-STORIES.md](USER-STORIES.md).

## User story

I want clipboard contents available across machines, including small images, without building a separate transfer system first.

## Situation and flow

An external clipboard bridge publishes contents; another consumes and may apply them locally. This story makes inline binary support useful. Large transfers and streaming remain deferred.

Clipboard bridge → illustrative `/desktop/clipboard` → remote bridge or viewer. Text uses a string; small images can use bytes with an application-defined format descriptor. Exact record shape is not selected.

Topic paths, payload layouts and timing examples are illustrative unless explicitly agreed elsewhere. This is an application story, not a mandatory global topic convention.

## Observable acceptance criteria

- **US-06.A1:** A supported small binary payload survives publication and consumption without byte changes through JSON and MessagePack paths.
- **US-06.A2:** A clipboard record stays one value, including its format fields.
- **US-06.A3:** The bridge can inspect provenance and avoid endlessly republishing a value it just applied; the chosen suppression rule belongs to the bridge.
- **US-06.A4:** Payloads exceeding the selected limit produce an explicit error rather than silent truncation.

## Details to learn through use

Manual pull versus automatic sync, MIME/format representation, expiry/persistence, size limit and loop suppression. No large blob registry or P2P transfer is implied.

## Implementation and evidence

- Core test links: [test plan](../test-plan.md) — U1 and T1; add text/image round-trip and bridge feedback-loop fixtures.
- Story-specific test file/command: not yet implemented. Map each criterion to a test or explicitly mark it manual/deferred.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.

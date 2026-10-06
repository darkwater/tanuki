# US-12 — Ad hoc local and remote scripts

- **Status:** simulated HTTP write/read implemented; real-world use and Unix socket not yet verified.
- **Origin:** User use case.
- **Last updated:** 2026-10-07.
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

The [client API proposal](../client-api-design.md) now focuses on explicit
WebSocket primitives and observation helpers for Rust scripts. HTTP SDK work
is deferred; existing HTTP examples and acceptance criteria remain unchanged.
Native SDK delivery now has loopback evidence; Unix-socket and deployment
verification remain absent.

## Implementation and evidence

- Native SDK evidence: ordinary `publisher`, `dashboard` and `controller` examples were compiled and exercised on loopback against the production binary. `tests/native_sdk.rs` covers raw wire control, typed freeform values, warnings, server rejection and mixed codecs. Existing stateless HTTP criteria remain covered by the server suites; the SDK adds no HTTP or Unix-socket transport.

- Core test links: [test plan](../test-plan.md) — C1, S1, T1; HTTP task 04; add socket parity tests when that adapter lands.
- `tests/http_api.rs::stateless_battery_publish_and_anonymous_read_cross_the_real_router`
  verifies US-12.A1 and US-12.A2 through the production router.
- `tests/http_api.rs::stateless_attribution_accepts_query_and_rejects_conflicts`
  verifies that a small client may use either `tanuki-client` or `?client=` and
  that ambiguous attribution is rejected.
- `tests/core_schema.rs`,
  `tests/http_api.rs::schema_installation_casts_valid_http_writes_and_denies_invalid_ones`,
  and `tests/websocket_api.rs::websocket_writes_cannot_bypass_an_installed_schema`
  verify US-12.A4 at the core, HTTP, and WebSocket boundaries.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.
- 2026-10-06: HTTP attribution gained a query-parameter alternative for clients
  that cannot conveniently set headers; no actual phone automation is verified.
- 2026-10-06: Task 09 made denying schemas consistent across the implemented
  core, HTTP, and WebSocket entrypoints.

- 2026-10-07: Native SDK delivery added typed WebSocket clients and raw/latest-state observation tests, plus compiled loopback examples. Browser bindings and Iced integration remain deferred; no real hardware/deployment verification was added.

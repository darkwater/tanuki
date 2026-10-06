# US-02 — Room location from motion

- **Status:** simulated motion-to-location-to-lamp flow implemented; real sensors and inference not yet verified.
- **Origin:** User use case.
- **Last updated:** 2026-10-06.
- **Lifecycle:** maintained under [USER-STORIES.md](USER-STORIES.md).

## User story

I want to estimate which area of my single room I occupy, so automation can respond to entering the kitchen, desk or couch area.

## Situation and flow

Several motion producers feed a location script. The kitchen is an area, not necessarily a separate room. The script consumes sensor state and publishes derived state; a second script can use it to control lighting.

Sensors → `/smart-home/entities/{sensor-kitchen,sensor-desk,sensor-couch}/motion` → location script → illustrative `/location/room` → automation → lamp input. The inference algorithm is application logic.

Topic paths, payload layouts and timing examples are illustrative unless explicitly agreed elsewhere. This is an application story, not a mandatory global topic convention.

## Observable acceptance criteria

- **US-02.A1:** The script obtains an initial coherent selection and reacts to sensor updates.
- **US-02.A2:** An atomic sensor batch is applied completely before the script recomputes its estimate.
- **US-02.A3:** An automation consuming the estimate can submit a lamp request, and a mock lamp controller can publish actual state for a dashboard.
- **US-02.A4:** Missing or old sensor data is visible to the script; Tanuki does not silently label missing motion data as false.
- **US-02.A5:** A deterministic fixture demonstrates sensor input → estimate → request → actual output.

## Details to learn through use

Actual sensors, inference and timeout rules, confidence/unknown representation, light-off policy and manual override behaviour. Do not invent these as core features.

## Implementation and evidence

- Core test links: [test plan](../test-plan.md) — C2, C3; E1 step 3; add a story-specific missing-sensor case.
- `tanuki/tests/websocket_api.rs::simulated_room_actors_drive_downstream_outputs_across_transports` covers the basic US-02.A1/A3/A5 chain with one motion state, an external location actor, an automation actor, and a lamp controller. Multi-sensor atomic recomputation and missing/stale sensor handling remain deferred.
- Real clients, scripts, configuration and deployment: not yet recorded.
- Observed behaviour and limitations: not yet verified. Passing mock-client tests alone does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.
- 2026-10-06: Task 06 exercised the external actor chain over real loopback transports; the inference fixture is intentionally simple and is not evidence from physical sensors.

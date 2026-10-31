# US-03 — TV state and sidecar display

- **Status:** volume bridge implemented with simulated-TV/real-server tests; source/power sidecar and hardware deployment remain unverified.
- **Origin:** User use case.
- **Last updated:** 2026-10-31.
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
- Story-specific tests: `cargo test -p tanuki-tv-volume`; A5–A7 coverage is detailed below. A1–A4 remain deferred.
- Real client: the TV volume example below, derived from `~/tv-volume.sh`; configured addresses and hardware deployment are not yet verified.
- Observed behaviour: volume publication/control is tested with a simulated TV and production Tanuki. Passing these tests does not establish real deployment.

## Change notes

- 2026-10-05: Initial story derived from the design conversation. Preserve the goal while refining concrete usage with the user.

## Volume bridge increment — 2026-10-31

The user requested both observation and control over Tanuki after discussing
`~/tv-volume.sh` and UPnP GENA updates. The concrete client is now
[`examples/tv-volume`](../../examples/tv-volume/README.md), named
`tv-volume bridge`: `/tv/volume` is observed retained integer state and
`/tv/desired-volume` is requested retained integer input (0–100). No schema is
automatically installed. Description/callback addresses are configured per
installation; no actual TV IP, model, firmware or successful deployment is known.

- **US-03.A5:** A volume consumer gets initial observed state and subsequent
  physical-remote changes through TV notifications, without volume polling.
- **US-03.A6:** A desired-volume submission sends SetVolume; SOAP acceptance
  alone must not change observed volume. Invalid values do not reach the TV.
- **US-03.A7:** Subscription/session failure removes observed volume when
  possible; a crashed bridge's value expires. A supervised restart subscribes
  anew and can apply still-unexpired retained intent.

A5/A6 have simulated-TV/production-server coverage in
`examples/tv-volume/tests/bridge.rs`. A7 covers renewal failure, sequence gaps,
claim takeover and orderly cleanup there; actual crash expiry and supervised
hardware restart are manual acceptance work, not verified deployment. Parser
unit tests exercise Master channel/instance 0, ranges and discovery URL rules.
A1–A4 remain outside this volume increment: power, HDMI, toggle and the tablet
projection have not been implemented by this example.

The example requires an initial Master volume event. It refreshes its 90-second
output lease on subscription renewals, which establish subscription health but
cannot prove a TV has delivered every physical change. No polling fallback,
continuous desired/actual reconciliation or automatic SSDP selection is added.

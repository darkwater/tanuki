# US-03 — TV state and sidecar display

- **Status:** volume bridge implemented with simulated-TV/real-server tests; read-only LAN discovery verified; source/power sidecar and hardware controls remain unverified.
- **Origin:** User use case.
- **Last updated:** 2026-11-01.
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
- Real client: the TV volume example below, derived from `~/tv-volume.sh`; LAN discovery is verified below; physical volume control/event delivery and service deployment remain unverified.
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
cannot prove a TV has delivered every physical change. No polling fallback or continuous desired/actual reconciliation is added.
Automatic SSDP selection was added in the next increment below.

## Address-free discovery — 2026-11-01

- **US-03.A8:** The volume service discovers a TV without a configured IP,
  selects it by stable UDN or unique friendly name when several renderers exist,
  and chooses a reachable local callback address. A read-only listing must not
  connect to Tanuki, apply retained volume intent or send volume commands.

Implemented as `--discover`, no-argument startup for one renderer, and
`--device UDN_OR_NAME`. The default IPv4 multicast route is searched on every
startup. Discovery and automatic callback tests live in
`examples/tv-volume/src/discovery.rs`; binary argument tests cover read-only mode
and explicit-URL compatibility. Duplicate devices are grouped by UDN. Multiple
matches require an explicit choice; no arbitrary device is controlled.

Read-only LAN verification found `[TV] Samsung TV`,
UDN `uuid:b79e314d-c9c0-4dc1-8ac3-1c46fee17138`, description
`http://192.168.0.152:9197/dmr`, plus renderers `Sinon Satellite` and `Tetsuya`.
Only SSDP searches and HTTP GETs were performed. The user explicitly required
that the playing TV's volume not be set during testing; the live bridge was
not launched. Physical-TV notifications, controls, callback reachability and
systemd deployment remain unverified. Discovery does not span VLANs or enumerate
multiple network interfaces; explicit callback/URL overrides remain available.

## Samsung empty-event regression — 2026-11-01

The user reported a running subscription to
`http://192.168.0.152:9197/upnp/event/RenderingControl1`, then an `empty LastChange`
warning followed by a fatal sequence-gap error. This establishes that a real
subscription was active; it does not by itself verify all control/deployment
criteria. The failure reproduces with one empty notification, without requiring
queue saturation. The parser rejected that event before its sequence could
reach the subscription loop.

US-03.A5 now has regression coverage for empty `LastChange` forms and a
130-notification sequential burst followed by a valid volume update. No-op events
preserve observed volume while advancing SEQ. Genuine lost events still follow
the documented restart policy. User-assisted receive-only verification accepted over 300 notifications using
the fixed parser, including many no-op events, while the user changed volume.
It also captured out-of-order pairs 78/77 and 169/168, prompting a bounded
one-second/64-event reorder buffer with unit and real-server regression tests.
The probe renewed successfully and unsubscribed with HTTP 200 on shutdown.
It used no Tanuki session or SOAP commands; all live volume changes came from
the user. This verifies notification parsing on the physical TV, while complete
bridge operation under reordering is tested with simulated-TV/production-server
fixtures. No volume-setting live test is authorized while media is playing.

## Mute state and control — 2026-11-01

- **US-03.A9:** Consumers can observe `/tv/mute` as a boolean and request mute
  or unmute through `/tv/desired-mute`. Mute is observed independently of volume;
  SOAP acceptance alone does not establish the actual mute state. Combined
  volume/mute notifications must be visible atomically.

The user requested mute alongside the implemented volume bridge. Both new
paths are retained, with `/tv/desired-mute` defined/claimed by `tv-volume bridge`.
Inputs require boolean payloads. The adapter uses RenderingControl `SetMute`
(instance 0, Master, DesiredMute 1/0); no toggle/replay command was introduced.
Observed mute remains absent until reported, expires after 90 seconds, and is
refreshed alongside known volume on subscription renewal. Mute-only events leave
volume unchanged; reordering preserves the latest value of each field.

Parser and production-server/simulated-TV tests cover initial absence, combined
atomic publication, mute-only events, true/false controls without optimistic
output, invalid input, per-input deduplication, claim takeover with an invalid
pending payload, renewal and atomic cleanup. A9 is implemented/tested with fake
TVs. Physical mute notifications/controls remain unverified: no live mute or
volume commands were sent while media was playing. Existing paths and the
single-TV discovery/selection conventions remain unchanged.

# TV volume bridge

A native Rust example service that publishes UPnP RenderingControl volume and
mute state to Tanuki and accepts volume/mute requests through Tanuki. It uses
GENA notifications, without volume polling. Developed from the user's `~/tv-volume.sh` Samsung
`SetVolume` script; tested against a simulated TV and the real Tanuki server.
Read-only SSDP discovery and volume notification parsing were verified on the
user's LAN with a receive-only probe on 2026-11-01. Physical-TV
volume/mute controls and complete bridge deployment remain unverified.

| Topic | Kind | Value |
| --- | --- | --- |
| `/tv/volume` | retained output | observed integer 0–100, instance 0, Master channel |
| `/tv/desired-volume` | retained input | requested integer 0–100 |
| `/tv/mute` | retained output | observed boolean: `true` means muted |
| `/tv/desired-mute` | retained input | requested boolean: `true` mutes, `false` unmutes |

The managed client name is `tv-volume bridge`. Paths and name are local example
conventions, currently fixed for one TV. Use a single running instance. No
schema is installed automatically: the bridge validates control values and logs
invalid requests. Tanuki acceptance does not guarantee TV execution. SOAP
success never publishes invented observed volume or mute state. Mute is not
inferred from a zero volume value.

## Run

Start Tanuki and discover available devices:

```sh
cargo run -p tanuki
# In another terminal: read-only listing; never connects to Tanuki or sends SOAP.
cargo run -p tanuki-tv-volume -- --discover
```

Discovery sends two SSDP `M-SEARCH` requests to `239.255.255.250:1900`, waits four
seconds for replies, then fetches their advertised description URLs using HTTP
GET. It keeps devices with a supported RenderingControl service and deduplicates
repeated advertisements/URLs by device UDN. The listing includes friendly name,
stable UDN and current description URL. Routers and other unrelated UPnP devices
are excluded.

Run without arguments if exactly one renderer is present, or choose one by its
exact name or stable UDN. Selecting by UDN survives DHCP address changes and
renaming. If multiple renderers match, startup lists the choices and exits
instead of choosing one arbitrarily.

```sh
cargo run -p tanuki-tv-volume
# Or choose the user's TV discovered on 2026-11-01:
cargo run -p tanuki-tv-volume -- --device uuid:b79e314d-c9c0-4dc1-8ac3-1c46fee17138
# Friendly names also work:
cargo run -p tanuki-tv-volume -- --device '[TV] Samsung TV'
```

These bridge commands enable the existing control behavior, including applying
unexpired retained desired volume or mute at startup. **Use `--discover` for read-only
checks while media is playing.** It returns before listener setup, Tanuki
connection, input claiming or control code. Automated control tests use isolated
loopback simulated TVs.

The callback address is chosen automatically from this machine's route to the
selected TV, using the actual bound listener port and `/notify`. No UDP datagram
is sent to determine that local address. Defaults are listener `0.0.0.0:9198` and
Tanuki `ws://127.0.0.1:5167/v1/ws`. Override these when necessary:

```sh
cargo run -p tanuki-tv-volume -- --device '[TV] Samsung TV' \
  --listen 0.0.0.0:9198 --tanuki-url ws://127.0.0.1:5167/v1/ws
# Routed/NAT setups can supply --callback-url http://REACHABLE_HOST:9198/notify.
```

Discovery uses the default IPv4 multicast route; it does not enumerate every
network interface or search across VLANs. An offline TV or blocked multicast
produces no match. Every supervised restart discovers again. Explicit device
and callback URLs remain supported for networks where discovery is unavailable:

```sh
cargo run -p tanuki-tv-volume -- \
  http://192.168.1.50:9197/description.xml \
  http://192.168.1.20:9198/notify \
  0.0.0.0:9198 ws://127.0.0.1:5167/v1/ws
```

These explicit addresses are placeholders. The user's actual read-only discovery
found `[TV] Samsung TV` at `http://192.168.0.152:9197/dmr` on 2026-11-01, alongside
`Sinon Satellite` and `Tetsuya`. The UUID selector above avoids depending on that
TV IP. No live bridge, subscription or volume command was run during this check.

The bridge reads the first RenderingControl v1/v2/v3 service's `controlURL` and
`eventSubURL`, resolving relative paths and optional `URLBase`. It does not guess
ports or event paths. Both TV URLs and the callback must use HTTP. The callback
path must be `/notify`, without a query, and its advertised port must route to
the listener. HTTP requests bypass proxy environment variables. Description
fetches during discovery have three-second timeouts, eight concurrent requests,
and the existing 64 KiB body limit; at most 512 SSDP responses and 64 description
URLs are processed.

Observe changes with the inspector or SSE, and submit requested audio settings:

```sh
cargo run -p tanuki-ui
curl -N 'http://127.0.0.1:5167/v1/sse?select=/tv/**'
curl -sS -H 'Tanuki-Client: TV remote' -H 'Content-Type: application/json' \
  --data '{"operations":[{"op":"submit_desired","topic":"/tv/desired-volume","value":25,"expiry":{"mode":"set","duration":"PT30S"}}]}' \
  http://127.0.0.1:5167/v1/write
# Mute (send false instead of true to unmute):
curl -sS -H 'Tanuki-Client: TV remote' -H 'Content-Type: application/json' \
  --data '{"operations":[{"op":"submit_desired","topic":"/tv/desired-mute","value":true,"expiry":{"mode":"set","duration":"PT30S"}}]}' \
  http://127.0.0.1:5167/v1/write
```

Desired mute requires a JSON boolean, not a numeric or string value. The bridge
calls `SetMute` with instance 0, Master channel and `DesiredMute` 1/0. Its SOAP
response confirms only action acceptance; observed `/tv/mute` changes only when
a TV notification reports it. Mute-only events leave volume unchanged. An event
containing volume and mute publishes both in one Tanuki batch, and reordered
event coalescing preserves the latest value of each field independently.

An unexpired retained request is applied once when the bridge starts/restarts.
Subsequent submissions are applied once per observed write provenance; rapid
requests can coalesce to the latest value. Metadata changes do not repeat a
request. It does not continually fight physical-remote changes to maintain the
old desired value. Clearing/expiring intent does not undo the TV setting. SOAP
failure is logged without an immediate retry; a later service restart can apply
still-retained intent again. Give short-lived requests an expiry as above.

## Lifetime and failure behavior

Startup discovers/selects a device (unless explicitly configured), chooses the
callback, fetches the description, connects to Tanuki, removes old observed state,
and defines/claims both desired inputs. The callback listener starts before
`SUBSCRIBE`, so an initial notification arriving before the subscribe response
can queue safely. The bridge waits for real subscription events and requires a
Master volume within ten seconds. TVs that omit it are currently unsupported;
there is no silent polling fallback or `GetVolume` bootstrap. Mute is optional
in the initial report: `/tv/mute` remains absent until reported and is never
defaulted to false. TV support for mute notifications and `SetMute` is required
to use those features; unsupported SOAP actions are logged without retry.

Subscriptions request 60 seconds and renew at half the granted interval, capped
at 30 seconds. Only the current SID is processed. Empty `LastChange` values
(including whitespace or empty CDATA) are accepted as no-op notifications: they
advance the subscription sequence without changing observed audio state. Duplicate
or late sequence numbers are ignored. Out-of-order notifications wait in a
buffer of at most 64 events for up to one second, then apply in sequence order;
contiguous buffered audio changes can coalesce to the latest value per field. An
unresolved gap or full reorder buffer ends the service so restarting gets a
fresh initial state. A bounded 64-message callback queue returns HTTP 503 when
full; malformed events return 400. HTTP bodies are limited to 64 KiB and outgoing
requests have ten-second timeouts. Callback replies use UPnP HTTP status codes,
not Tanuki JSON envelopes.

Observed volume and mute have 90-second expiries, refreshed when each field is
reported and together on successful subscription renewal (for known fields).
A renewal refresh repeats the last observed values; their Tanuki timestamps
indicate bridge/subscription health, **not a new measurement**.
A TV that silently stops delivering events while accepting renewals cannot be
detected without additional device-specific checks. This is an explicit limit
of this notification-only example.

Renewal failure, event gaps, Tanuki disconnection, or observed claim takeover
end the service with an error. Ctrl-C/SIGTERM ends it normally. Cleanup attempts
`UNSUBSCRIBE`, stops the callback task, removes both observed topics in one
batch and closes the session, releasing both claims. Desired intent remains.
If cleanup cannot reach Tanuki, the published audio states expire. The bridge stores nothing on disk;
Tanuki owns any persistence. Callback and control operations are serialized in
the bridge loop; a slow SOAP request can delay processing/renewal until timeout.

## Run as a user service

Build with `cargo build --release -p tanuki-tv-volume`. Copy and edit
[tv-volume.service](tv-volume.service) into
`~/.config/systemd/user/tanuki-tv-volume.service`, using the actual executable and the stable TV UDN from `--discover`. The template
automatically discovers its current address and chooses the callback. The unit is a template, not
installed by this example.

```sh
systemctl --user daemon-reload
systemctl --user enable --now tanuki-tv-volume
journalctl --user -u tanuki-tv-volume -f
```

The unit retries failures after five seconds, including TV-off and Tanuki-down
startup. Each restart establishes a fresh subscription and explicitly reclaims
intent; it is not session resumption. Normal shutdown is not restarted.

## Verify

```sh
cargo test -p tanuki-tv-volume
cargo test -p tanuki-tv-volume --release
```

Unit tests cover XML channel/instance selection, malformed/range checks,
URL resolution and granted subscription duration. Discovery tests use a loopback
SSDP responder and HTTP descriptions to verify duplicate suppression, device
filtering, GET-only access, unique name/UDN selection and callback route/port
selection; CLI tests cover read-only listing and existing explicit-address syntax. Integration tests use actual
HTTP callbacks, SOAP requests, WebSockets and production Tanuki server logic:
initial/event publication, atomic volume/mute updates, mute-only and reordered
audio changes, SetMute for true/false without optimistic output, invalid intent,
foreign subscriptions, renewal, a 130-event empty-notification burst followed by
a valid volume update, reordered updates, duplicate/wraparound handling,
missing-event recovery boundary, claim takeover,
shutdown and stale-state removal. Hardware acceptance remains manual: subscribe,
press the physical remote, submit Tanuki intent, switch the TV off/on and confirm
recovery through the configured supervisor.

References: [UPnP event subscriptions](https://www.upnp.org/specs/arch/UPnP-arch-DeviceArchitecture-v1.0-20080424.pdf)
and [RenderingControl LastChange](https://upnp.org/specs/av/UPnP-av-RenderingControl-v1-Service.pdf).

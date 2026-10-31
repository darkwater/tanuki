# TV volume bridge

A native Rust example service that publishes UPnP RenderingControl volume to
Tanuki and accepts volume requests through Tanuki. It uses GENA notifications,
not volume polling. Developed from the user's `~/tv-volume.sh` Samsung
`SetVolume` script; tested against a simulated TV and the real Tanuki server.
Read-only SSDP discovery was verified on the user's LAN on 2026-11-01;
physical-TV volume control and event delivery remain unverified.

| Topic | Kind | Value |
| --- | --- | --- |
| `/tv/volume` | retained output | observed integer 0–100, instance 0, Master channel |
| `/tv/desired-volume` | retained input | requested integer 0–100 |

The managed client name is `tv-volume bridge`. Paths and name are local example
conventions, currently fixed for one TV. Use a single running instance. No
schema is installed automatically: the bridge validates control values and logs
invalid requests. Tanuki acceptance does not guarantee TV execution. SOAP
success never publishes an invented actual volume.

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
unexpired retained desired volume at startup. **Use `--discover` for read-only
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

Observe changes with the inspector or SSE, and submit a requested volume:

```sh
cargo run -p tanuki-ui
curl -N 'http://127.0.0.1:5167/v1/sse?select=/tv/**'
curl -sS -H 'Tanuki-Client: TV remote' -H 'Content-Type: application/json' \
  --data '{"operations":[{"op":"submit_desired","topic":"/tv/desired-volume","value":25,"expiry":{"mode":"set","duration":"PT30S"}}]}' \
  http://127.0.0.1:5167/v1/write
```

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
and defines/claims the desired input. The callback listener starts before
`SUBSCRIBE`, so an initial notification arriving before the subscribe response
can queue safely. The bridge waits for real subscription events and requires a
Master volume within ten seconds. TVs that omit it are currently unsupported;
there is no silent polling fallback or `GetVolume` bootstrap.

Subscriptions request 60 seconds and renew at half the granted interval, capped
at 30 seconds. Only the current SID is processed. Empty `LastChange` values
(including whitespace or empty CDATA) are accepted as no-op notifications: they
advance the subscription sequence without changing observed volume. Duplicate
or late sequence numbers are ignored. Out-of-order notifications wait in a
buffer of at most 64 events for up to one second, then apply in sequence order;
contiguous buffered volume changes can coalesce to the latest value. An
unresolved gap or full reorder buffer ends the service so restarting gets a
fresh initial state. A bounded 64-message callback queue returns HTTP 503 when
full; malformed events return 400. HTTP bodies are limited to 64 KiB and outgoing
requests have ten-second timeouts. Callback replies use UPnP HTTP status codes,
not Tanuki JSON envelopes.

Actual volume has a 90-second expiry, refreshed on events and successful
subscription renewal. A renewal refresh repeats the last observed value; its
Tanuki timestamp indicates bridge/subscription health, **not a new measurement**.
A TV that silently stops delivering events while accepting renewals cannot be
detected without additional device-specific checks. This is an explicit limit
of this notification-only example.

Renewal failure, event gaps, Tanuki disconnection, or observed claim takeover
end the service with an error. Ctrl-C/SIGTERM ends it normally. Cleanup attempts
`UNSUBSCRIBE`, stops the callback task, removes observed volume and closes the
session, releasing its claim. Desired intent remains. If cleanup cannot reach
Tanuki, the published volume expires. The bridge stores nothing on disk;
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
initial/event publication, control without optimistic output, invalid intent,
foreign subscriptions, renewal, a 130-event empty-notification burst followed by
a valid volume update, reordered updates, duplicate/wraparound handling,
missing-event recovery boundary, claim takeover,
shutdown and stale-state removal. Hardware acceptance remains manual: subscribe,
press the physical remote, submit Tanuki intent, switch the TV off/on and confirm
recovery through the configured supervisor.

References: [UPnP event subscriptions](https://www.upnp.org/specs/arch/UPnP-arch-DeviceArchitecture-v1.0-20080424.pdf)
and [RenderingControl LastChange](https://upnp.org/specs/av/UPnP-av-RenderingControl-v1-Service.pdf).

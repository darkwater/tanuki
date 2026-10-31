# TV volume bridge

A native Rust example service that publishes UPnP RenderingControl volume to
Tanuki and accepts volume requests through Tanuki. It uses GENA notifications,
not volume polling. Developed from the user's `~/tv-volume.sh` Samsung
`SetVolume` script; tested against a simulated TV and the real Tanuki server.
No physical TV has been verified yet.

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

Start Tanuki, find the TV's device-description URL, and choose this machine's
LAN address (reachable **from the TV**):

```sh
cargo run -p tanuki
# In another terminal; substitute actual addresses and description path:
cargo run -p tanuki-tv-volume -- \
  http://192.168.1.50:9197/description.xml \
  http://192.168.1.20:9198/notify \
  0.0.0.0:9198 \
  ws://127.0.0.1:5167/v1/ws
```

The description URL above is a placeholder, not a known Samsung endpoint.
Use the `LOCATION` URL from the TV's SSDP advertisement/search response. For
example, this Python standard-library snippet prints device-description URLs
advertised by UPnP devices on the local network (identify the TV by responder IP):

```sh
python3 - <<'PY'
import socket, time
with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
    s.settimeout(0.5)
    s.sendto(b'M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: "ssdp:discover"\r\nMX: 2\r\nST: upnp:rootdevice\r\n\r\n', ('239.255.255.250', 1900))
    until = time.monotonic() + 4
    seen = set()
    while time.monotonic() < until:
        try:
            data, address = s.recvfrom(65535)
        except socket.timeout:
            continue
        for line in data.decode(errors='replace').splitlines():
            if line.lower().startswith('location:'):
                item = (address[0], line.split(':', 1)[1].strip())
                if item not in seen:
                    print(*item)
                    seen.add(item)
PY
```

The bridge reads the first RenderingControl v1/v2/v3 service's `controlURL` and
`eventSubURL`, resolving relative paths and optional `URLBase`. It does not guess
ports or event paths. Both TV URLs and the callback must use HTTP. The callback
path must be `/notify`, without a query, and its advertised port must route to
the listener. Binding defaults to `0.0.0.0:9198`; the Tanuki URL defaults to the
loopback URL shown above. HTTP requests bypass proxy environment variables.

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

Startup fetches the description, connects to Tanuki, removes old observed state,
and defines/claims the desired input. The callback listener starts before
`SUBSCRIBE`, so an initial notification arriving before the subscribe response
can queue safely. The bridge waits for real subscription events and requires a
Master volume within ten seconds. TVs that omit it are currently unsupported;
there is no silent polling fallback or `GetVolume` bootstrap.

Subscriptions request 60 seconds and renew at half the granted interval, capped
at 30 seconds. Only the current SID is processed. Duplicate sequence numbers
are ignored; gaps or out-of-order numbers end the service so restarting gets a
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
`~/.config/systemd/user/tanuki-tv-volume.service`, using the actual executable,
TV description, callback and Tanuki addresses. The unit is a template, not
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
URL resolution and granted subscription duration. Integration tests use actual
HTTP callbacks, SOAP requests, WebSockets and production Tanuki server logic:
initial/event publication, control without optimistic output, invalid intent,
foreign subscriptions, renewal, missing-event recovery boundary, claim takeover,
shutdown and stale-state removal. Hardware acceptance remains manual: subscribe,
press the physical remote, submit Tanuki intent, switch the TV off/on and confirm
recovery through the configured supervisor.

References: [UPnP event subscriptions](https://www.upnp.org/specs/arch/UPnP-arch-DeviceArchitecture-v1.0-20080424.pdf)
and [RenderingControl LastChange](https://upnp.org/specs/av/UPnP-av-RenderingControl-v1-Service.pdf).

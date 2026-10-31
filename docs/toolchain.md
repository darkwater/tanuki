# Toolchain

Tanuki uses `nightly-2026-10-01` with rustfmt and Clippy, pinned in
`rust-toolchain.toml`. The pin was verified on 2026-10-05 with:

```text
rustc 1.101.0-nightly (21b707e3f 2026-09-30)
cargo 1.101.0-nightly (f3865b2a4 2026-09-29)
```

Runtime dependencies now include Tokio 1.53.2, Axum 0.8.9, Serde/serde_json,
MessagePack via rmp-serde 1.3.1, Jiff 0.2.37, thiserror 2.0.21, base64 0.23.1,
tracing 0.1.44, and tracing-subscriber 0.3.23. Tower 0.5.3 is used by router
integration tests; futures-util 0.3.34 and Tokio Tungstenite 0.29.0 drive real
WebSocket test clients. Versions are exact pins in the package `Cargo.toml` manifests; `Cargo.lock`
is committed. Tokio Tungstenite matches Axum's transitive version to avoid a
duplicate WebSocket stack in the test build.

## Unstable features

No crate-level unstable features are enabled yet. Nightly is pinned so future
features can be selected and verified deliberately. When adding one, record
its feature gate, purpose, and links to the pinned standard-library or
Unstable Book documentation here.

## Quality gates

Run these commands from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --release
```

The default toolchain selected by each command comes from
`rust-toolchain.toml`.

## Native SDK verification — 2026-10-07

The workspace adds `tanuki-protocol` and `tanuki-client` on the existing nightly
pin. Protocol has Serde/JSON, MessagePack, base64, Jiff and thiserror runtime dependencies;
its dependency graph contains no Tokio, sockets, server or filesystem facilities.
Client reuses pinned Tokio/Tungstenite 0.29.0 and futures-util 0.3.34, with concrete
native adapters. No async-trait, dynamic transport framework, new feature gates
or nightly upgrade was needed. Ordinary Serde conversion uses serde-value 0.7.0;
serde_bytes 0.11.19 is a test/example integration recommendation. The lockfile is
updated with their resolved dependencies.

The 2026-10-07 API/protocol review enabled serde_json's `float_roundtrip`
feature in the protocol crate. Cargo feature unification also enables it in
the client and server; versions and the nightly pin are unchanged. The pinned
serde_json implementation's feature-gated `f64_from_parts` paths were checked
locally. A deterministic bit-pattern regression reproduced a one-bit change
for `2.291712365432881e-9` before enabling the feature. Tests cover signed zero,
subnormals, maximum finite values, and 10,000 generated bit patterns through
JSON and MessagePack. No additional crate or unstable feature was needed.
The already pinned rmp-serde dependency now also serves the protocol crate at
runtime. Its `Deserializer::new`/`get_ref` APIs were checked in the pinned
source: the remaining input slice lets `decode_messagepack` reject trailing
data, which `rmp_serde::from_slice` otherwise silently ignores. This adds no
transport dependency to the shared crate.

API checks: [Tungstenite connect configuration](https://docs.rs/tokio-tungstenite/0.29.0/tokio_tungstenite/fn.connect_async_with_config.html),
[Tokio watch receiving/version semantics](https://docs.rs/tokio/1.53.2/tokio/sync/watch/struct.Receiver.html),
[serde-value variants](https://docs.rs/serde-value/0.7.0/serde_value/enum.Value.html),
and [Serde's internally tagged decoder](https://docs.rs/serde_derive/latest/src/serde_derive/de/enum_internally.rs.html).
Pinned Serde source confirms its buffered ContentDeserializer does not retain
`is_human_readable`. Behavioral fixtures justify streaming DTO decoding.
serde-value does not implement 128-bit integer serialization; use explicit i64
narrowing or Value access for that input, and no semantic time autodetection is
provided. Native transport currently has no TLS feature enabled.

The attempted `cargo check -p tanuki-protocol --target wasm32-unknown-unknown`
failed with E0463 because that target's standard library is not installed.
Only `x86_64-unknown-linux-gnu` is installed. This is not evidence of a code
portability failure or of browser support. Browser transport and bindings remain
outside this implementation. Native checks and standalone protocol compilation
pass on the pinned toolchain; `cargo test` defaults to all workspace members.


## Server boundary verification — 2026-10-07

The server review keeps the compiler and all dependency versions unchanged and
adds no unstable gates. Tokio watch `wait_for`/`send_modify` and join handles
provide cancellation, stream drainage, and scheduler ownership. A retained
Tokio `AbortHandle` also stops timers when the runtime owner is dropped while
other handles remain alive. Watch borrows
are discarded before awaiting socket work. Axum's native `Listener` interface
wraps accepted TCP I/O so a stalled peer cannot block shutdown forever; its
native accept implementation supplies retry/error handling. A single boxed
notification future at that I/O boundary preserves wakeups without a general
transport trait or async-trait dependency.

APIs were checked in the pinned dependency source and compiled against the pin:
[Tokio watch receiver](https://docs.rs/tokio/latest/tokio/sync/watch/struct.Receiver.html),
[Tokio watch sender](https://docs.rs/tokio/latest/tokio/sync/watch/struct.Sender.html),
[Axum listener](https://docs.rs/axum/latest/axum/serve/trait.Listener.html),
and [Tokio AsyncRead](https://docs.rs/tokio/latest/tokio/io/trait.AsyncRead.html).
The pages resolve to Tokio 1.53.2 and Axum 0.8.9 at verification time.

## Inspector Task 01 — 2026-10-31

`tanuki-ui` uses the existing `nightly-2026-10-01`, protocol crate and pinned
thiserror 2.0.21 (serde_json 1.0.151 only for test fixtures). Cargo.lock includes
the new package; no existing dependency versions change and no feature gates
are added. Tree/selection/formatting use ordinary standard-library collections,
string formatting and Unicode character iteration. Ratatui/Crossterm API/version
evaluation is deferred until terminal implementation; this task needs neither.

## Inspector Task 02 — 2026-10-31

The pin remains `nightly-2026-10-01`, with no new feature gates or dependency
versions. UI production dependencies now include the existing native client,
Tokio 1.53.2 and tracing 0.1.44; server and Jiff are development dependencies
for real-server tests. The workspace lockfile records these edges. Session/
RawListener APIs are verified in the pinned SDK source and by compiled tests;
no trait abstraction, macro async trait or boxed future is introduced. A
standard pending future keeps inactive receives dormant, and native Tokio
select/SDK raw receives provide cancellation at input/render boundaries.
Terminal dependencies and TLS remain outside this task.

## Inspector Task 03 — terminal dependencies, 2026-10-31

Pinned `nightly-2026-10-01` compiled Ratatui **0.30.2** and Crossterm **0.29.0**
without compiler upgrades or new feature gates. Ratatui is selected with default
features disabled and only the Crossterm backend enabled. Crossterm's
`event-stream` feature supplies async terminal events. Existing futures-util
0.3.34 supplies StreamExt; Tokio gains signal support for external SIGINT.
Cargo.lock includes the selected terminal dependency graph. Server/SDK/protocol
package versions remain unchanged.

API selection and checks used the official
[Ratatui installation/version pairing](https://ratatui.rs/installation/),
[Crossterm event API](https://docs.rs/crossterm/0.29.0/crossterm/event/index.html),
[EventStream](https://docs.rs/crossterm/0.29.0/crossterm/event/struct.EventStream.html),
and [Tokio JoinHandle cancellation behavior](https://docs.rs/tokio/1.53.2/tokio/task/struct.JoinHandle.html),
alongside compiled pinned-source APIs for Terminal, Frame, List/ListState,
Paragraph, TestBackend and Span width. Some umbrella Ratatui widget doc URLs
were unavailable; the downloaded modular crate source and compile checks
confirmed their re-exports and syntax. No unstable rendered-line API, custom
terminal framework, async-trait or boxed future was needed.

The PTY unwind probe builds a temporary Cargo package depending on the actual
UI library. A direct rustc attempt cannot consume this environment's cached
metadata-only rlib stubs; Cargo selects the corresponding compiled artifacts.
This is a smoke-tool implementation detail, not a compiler or application bug.

## Browser SDK — 2026-10-31

The unchanged `nightly-2026-10-01` now also has the installed
`wasm32-unknown-unknown` standard library, and protocol/client checks pass for that
target. This supersedes the earlier uninstalled-target limitation. Native Tokio
network/runtime/time features and Tungstenite are target-specific dependencies.
Browser dependencies are pinned Gloo Net 0.7.0 (WebSocket only), Gloo Timers 0.4.0
(futures), wasm-bindgen-futures 0.4.80 and web-time 1.1.0. Browser tests use
wasm-bindgen-test 0.3.80 and matching wasm-bindgen-cli 0.2.130. Cargo.lock records
the resolved browser graph; existing dependency versions and feature gates stay
unchanged. No unstable gates or compatibility macro/trait dependencies are added.

Standard Instant cannot supply browser monotonic time. API checks used the
[nightly Instant docs](https://doc.rust-lang.org/nightly/std/time/struct.Instant.html),
[Unstable Book](https://doc.rust-lang.org/nightly/unstable-book/),
[web-time](https://docs.rs/web-time/1.1.0/web_time/),
[Gloo WebSocket](https://docs.rs/gloo-net/0.7.0/gloo_net/websocket/futures/struct.WebSocket.html),
[local spawning](https://wasm-bindgen.github.io/wasm-bindgen/api/wasm_bindgen_futures/fn.spawn_local.html),
and pinned source for Gloo close/readiness, timer integer limits and Tokio's
executor-independent sync APIs. Compile checks against the pin and browser tests
verify the actual syntax and runtime behavior. Browser task cancellation uses
futures-util's Abortable and Tokio oneshot results; WASM panic aborts are not
recoverable task errors. Timer waits round up sub-millisecond waits and chunk
at signed 32-bit milliseconds, rechecking the monotonic deadline after waking.

Headless Chrome/ChromeDriver 156.0.8078.80 ran the SDK against the production
binary on loopback. Browser debug/release gates and setup live in
[testing.md](testing.md#browser-sdk-acceptance--2026-10-31) and CI. Browser tooling
is a developer/test prerequisite, not a shipped SDK runtime dependency. Native
examples and the terminal inspector remain native targets.

SDK README snippets use rustdoc's
[`standalone_crate`](https://doc.rust-lang.org/rustdoc/write-documentation/documentation-tests.html)
with `no_run`, so browser doc compilation does not attempt to execute a merged
WASM test harness as a native process. Both snippets compile against the pin on
native and WASM; no ignored examples or feature gates are needed.

## TV volume example — 2026-10-31

The unchanged nightly pin builds `examples/tv-volume`, a native workspace
application. Reqwest 0.13.5 (default features off; local HTTP only), URL 2.5.8 and
roxmltree 0.21.1 supply HTTP requests, URL resolution and XML parsing. Existing
Axum, Tokio, SDK, tracing and thiserror versions are reused; no unstable gates
or compatibility wrappers were added. Cargo.lock includes the new dependency
graph without upgrading existing packages. The example is not a browser target.

API checks used [Reqwest redirect policy](https://docs.rs/reqwest/0.13.5/reqwest/redirect/),
[roxmltree Document](https://docs.rs/roxmltree/0.21.1/roxmltree/struct.Document.html),
and pinned source for `Node::has_tag_name`, `Response::chunk` and SDK observers.
Named XML tags match regardless of namespace prefix. Parser defaults disallow
DTDs. Compile, Clippy and integration checks exercise the actual pinned APIs.

TV discovery (2026-11-01) reuses the existing pinned Tokio UDP sockets,
`lookup_host`, `timeout_at` and `JoinSet`, plus Reqwest/roxmltree. No dependencies,
compiler pin or unstable gates change. APIs were checked against
[Tokio UdpSocket](https://docs.rs/tokio/1.53.2/tokio/net/struct.UdpSocket.html)
and the UPnP Device Architecture discovery specification. UDP connect/local_addr
chooses a callback route without sending a datagram. Loopback compile/runtime
tests verify the pinned APIs; production discovery uses IPv4 multicast.

The empty-notification fix (2026-11-01) uses existing roxmltree text extraction
and standard `str::trim` to classify empty events. Compiler, dependency pins and
feature gates are unchanged; parser and loopback regression tests exercise the
same compiled bridge in debug and release modes. A standard `BTreeMap` and
Tokio `Instant`/`sleep_until` provide bounded notification reordering; no
additional dependency or runtime task is introduced.

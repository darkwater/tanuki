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

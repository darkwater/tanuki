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

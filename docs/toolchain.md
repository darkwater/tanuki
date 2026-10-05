# Toolchain

Tanuki uses `nightly-2026-10-01` with rustfmt and Clippy, pinned in
`rust-toolchain.toml`. The pin was verified on 2026-10-05 with:

```text
rustc 1.101.0-nightly (21b707e3f 2026-09-30)
cargo 1.101.0-nightly (f3865b2a4 2026-09-29)
```

Runtime dependencies now include Tokio 1.53.2, Axum 0.8.9, Serde/serde_json,
Jiff 0.2.37, thiserror 2.0.21, base64 0.23.1, tracing 0.1.44, and
tracing-subscriber 0.3.23. Tower 0.5.3 is used by router integration tests.
Versions are exact pins in `Cargo.toml`; `Cargo.lock` is committed.

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

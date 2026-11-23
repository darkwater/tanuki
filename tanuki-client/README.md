# tanuki-client

The user-facing Tanuki client API for Rust. **Hand-maintained.** It is
currently a placeholder that re-exports its foundation as
`tanuki_client::core`.

It builds on [`tanuki-client-core`](../tanuki-client-core/README.md), which owns
everything about the connection: hello, reconnects with backoff, request IDs
and reply correlation, timeouts, selection and setup replay, and resync when
the reader falls behind. The core hands this crate a cloneable `Client` for
writes and an `Events` stream of snapshots, updates and lifecycle events. State
management (caches, typed topics, observers) belongs here.

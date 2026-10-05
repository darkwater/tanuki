# Architecture

This is the living implementation map. `design.md` contains the design
proposal and review sketches; this document records what the code actually
does.

## Current module map

| Module | Status | Responsibility |
| --- | --- | --- |
| `server` | bootstrap | Own the process lifecycle and, later, runtime wiring and shutdown |
| `domain` | checkpoint A | Paths, selectors, values, identities, nodes, and operations |
| `core` | planned | Authoritative state, sessions, atomic commits, and subscriptions |
| `protocol` | planned | Versioned transport DTOs and codec conversion |
| `transport` | planned | Axum HTTP/WebSocket extraction and response mapping |
| `persistence` | planned | Versioned coherent snapshots and file operations |
| `schema` | planned | Ordinary-topic validation and freshness policy |
| `links` | planned | Linked path resolution, visibility, and recovery |
| `diagnostics` | planned | Structured warnings/errors and logging integration |

Only `server` exists in code. Domain and core modules wait for architecture
checkpoint A so their types do not accidentally freeze open product decisions.

## Dependency direction

The intended inward dependency flow is transport/protocol → core → domain.
Domain code will not depend on Axum, sockets, or disk. Persistence and network
I/O will happen outside the authoritative state transition. The first server
seam accepts an injectable shutdown future so tests exercise the production
lifecycle without process signals.

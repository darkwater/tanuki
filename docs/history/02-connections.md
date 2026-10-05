# Entrypoints and connection lifecycle

## Requirements and current direction

Different entrypoints may have different admission rules and interaction styles.

| Example | Admission | Interaction |
| --- | --- | --- |
| Local Linux endpoint | Access to a Unix socket, potentially tied to the user's account; no additional application authentication required | Local processes connect independently |
| Smartphone endpoint | HTTP; remote authentication details undecided | A periodic task makes a one-off update |
| Laptop publisher | Authentication details undecided | Keeps a connection open and streams updates |

The system should be able to expose that a laptop publisher lost its connection. Completion of a smartphone's one-off HTTP request must not imply that the smartphone became offline.

There can be multiple independent publishers on one physical machine. Neither authentication identity nor connection should force one process to represent the whole device.

## Latest direction: topic-level metadata

The user prefers exploring topic provenance and current ownership over a single explicit device-presence registration. See [topic metadata](03-topic-metadata.md).

The earlier proposal to call `track_presence("/devices/laptop")` is not the selected model. It risked recreating the one-process-per-device assumption.

## Proposed conceptual distinctions

These are design vocabulary, not settled protocol objects:

- **Entrypoint:** the interface and its admission policy.
- **Identity:** the caller recognized by that policy.
- **Connection:** a particular transport interaction.
- **Session:** a possible core-side lifetime for resources associated with a client.
- **Topic ownership:** possible responsibility for maintaining one topic while connected.

A shared identity may have several concurrent connections. A persistent connection may also perform writes with no ongoing ownership obligation; this is a proposal, not yet agreed.

## Proposed lifecycle

1. Establish transport.
2. Resolve identity and permissions using entrypoint policy.
3. If needed, create a session for ongoing resources.
4. Perform operations and maintain liveness.
5. End the session and update affected resources.

Proposed end causes: explicit close, detected transport failure, and a liveness timeout for silent disappearance. Exact deadlines, heartbeat mechanisms, cleanup behaviour, and protocol mappings remain open.

## Local endpoint topology — open

A Unix socket on each Linux machine might be served by a local forwarding agent, but this architecture has not been chosen.

If a forwarding agent is used, distinguish a local process disconnecting from the agent losing its upstream connection. Local publishers may still be running during an upstream outage. Multiplexing must not erase the information needed for independent ownership and failure reporting.

## Reconnection — proposal

Initially create a new session rather than resume an old one. Re-register resources as needed. Cleanup for an old connection must not invalidate resources now associated with a newer connection.

Resumption, takeover, buffering during outages, and duplicate connections are unresolved.

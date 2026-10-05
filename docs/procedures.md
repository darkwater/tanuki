# Runtime procedures

## Startup and shutdown

The bootstrap binary starts a Tokio runtime and waits for the process Ctrl-C
signal through `server::run_until`. Tests inject a one-shot shutdown future
through the same seam and join the spawned task.

There is no network listener, authoritative state, restore step, or background
work yet. As those arrive, startup will construct dependencies before exposing
the listener. Shutdown will stop accepting work, close producers, finish the
core task, request a best-effort save when applicable, and join owned tasks.
The exact procedure must be updated alongside each implementation card.

Mutation, subscription, disconnect/replacement, expiry, restore, and link
recovery procedures remain pending their corresponding architecture reviews.

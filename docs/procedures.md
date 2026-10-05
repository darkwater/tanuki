# Runtime procedures

## Startup and shutdown

The bootstrap binary starts a Tokio runtime and waits for the process Ctrl-C
signal through `server::run_until`. Tests inject a one-shot shutdown future
through the same seam and join the spawned task.

There is no network listener, restore step, or background work yet. The
authoritative in-memory `Core` is currently exercised directly. As runtime
wiring arrives, startup will construct dependencies before exposing the
listener. Shutdown will stop accepting work, close producers, finish the core
task, request a best-effort save when applicable, and join owned tasks. The
exact procedure must be updated alongside each implementation card.

Subscription, disconnect/replacement, expiry, restore, and link-recovery
procedures remain pending their corresponding implementation cards.

## Output mutation and atomic commit

All implemented output writes pass through `Core::apply`:

1. Reject reserved-system targets and repeated canonical targets before staging.
2. Clone authoritative nodes into a private candidate and compute the next
   sequence without modifying live state.
3. Apply operations in request order. Compute provenance from the supplied
   actor and explicit timestamp. Accumulate typed warnings and observable
   changes privately.
4. If any operation fails, discard the candidate, warnings, state changes, and
   event occurrences. The live sequence does not advance.
5. Otherwise install the whole candidate, advance the sequence once, and return
   one indivisible update batch plus successful-operation warnings.

No network or disk work occurs in this transition. Task 02 supports state/event
publication and removal. Input operations deliberately return a typed
unsupported-operation error until task 03 adds their complete ownership rules
to this same path.

# Tanuki terminal inspector

Milestone 1 is a read-only native terminal inspector. Start the server and UI
in separate terminals:

```sh
cargo run -p tanuki
cargo run -p tanuki-ui
cargo run -p tanuki-ui -- --url ws://127.0.0.1:5167/v1/ws --client desk-inspector
cargo run -p tanuki-ui -- --help
```

The left pane browses topics; the right shows the selected full path, kind,
value type/value, provenance, expiry and claims. Reserved system nodes are cyan
and marked `[sys]`. Connection/currentness and help hints are at the bottom.
Branches start collapsed. No publish, edit, claim, deletion or policy management
controls are provided. Schema details explicitly say they are unavailable.

| Key | Action |
| --- | --- |
| Up/Down or j/k | Select topic; in details, scroll rows |
| Left/Right or h/l | Collapse/expand or move to parent/child; in details, scroll columns |
| Tab | Switch pane |
| PageUp/PageDown | Scroll detail rows from either pane |
| Home/End | First/last topic; in details, start/end of value columns |
| / | Edit case-sensitive path filter; ancestors stay visible |
| Enter / Backspace | Accept filter / erase one filter character |
| Escape | Clear filter or close help |
| r | Explicit fresh-snapshot reconnect; ignored while already connecting |
| ? | Toggle help |
| q / Ctrl-C | Quit (q is text while editing a filter) |

Long detail lines are complete and can be scrolled horizontally; Home resets
both detail offsets. The scroll title counts Unicode scalar positions, while
Ratatui clips glyphs to terminal cells. Wide text remains reachable at End.
Windows below 60x10 show a resize/status/quit fallback; input still works.
Raw mode, alternate screen and cursor are restored by an owning guard on normal
exit, I/O errors and unwinding. Shell restoration precedes joined SDK cleanup.
Terminal I/O errors are printed only after restoration, with controls escaped.

Verification:

```sh
cargo test -p tanuki-ui
cargo build -p tanuki -p tanuki-ui
python3 tanuki-ui/tools/terminal-smoke.py
```

Cargo tests are headless and include real loopback servers. The separate POSIX
PTY smoke script runs actual binaries, seeds lamp state over HTTP, exercises
navigation/filter/help/resize, q, Ctrl-C key and SIGINT, and compares terminal
attributes before/after. It also checks output failure during setup and a
temporary Cargo probe unwinding through the production terminal guard. It uses
only Python stdlib and the pinned compiler and isolates persistence in `/tmp`.
It is local simulated use, not a deployed/hardware verification.

The following library APIs support the UI and headless consumers.

`Model::from_snapshot` validates protocol path strings and owns one
`SelectedView`. `Model::apply` validates all paths before applying one complete
batch; rejected paths/sequences leave the cache unchanged. Occurrences never
become retained values or history. Inspect `rows()`, `selected_path()` and
`selected_node()` after the mutation returns. Prefix rows are navigation only:
a path can have both a real node and descendants.

Selection uses full validated `TopicPath` identity. Branches begin collapsed;
`set_expanded` preserves expansion through updates while a prefix survives.
`set_filter` uses case-sensitive full-path substrings and reveals ancestor
context without overwriting saved expansion. Collapse prefers a visible
ancestor; removal otherwise chooses the nearest surviving old visible row,
with preceding rows winning ties. `move_selection` clamps at the ends.

`format::details` returns full logical lines, including value type, provenance,
expiry and claims. `format::value_text` preserves semantic time tags and shows
inline bytes as hex. `format::preview` escapes controls and shortens text to a
Unicode-scalar budget, including the ellipsis; it is not a terminal-cell width
calculation. Details are not truncated. A later renderer must provide scrolling
and cell clipping, use `safe_text` for paths/filter/status text, and never render
raw DTO strings. Formatting never modifies the underlying data.

`App` owns one SDK `Session` and raw listener, applying every complete batch
without an additional queue or observation task. Connection state is explicit:
connecting, live, disconnected, lagged or error. All states except live denote
non-current cached data. Abnormal receive results remove the listener and retain
the last complete cache; there are no retries, re-registration or replay.
`next_update()` remains pending when there is no listener, avoiding a busy loop.
It can be cancelled by a `tokio::select!` input/tick branch without dropping a
batch. Rendering can skip frames; the network branch must keep applying batches.

The default URL is `ws://127.0.0.1:5167/v1/ws`. The default client name is
`tanuki-ui-<pid>-<counter>`, distinct for simultaneous local processes and
multiple inspectors in one process. These names are not globally unique across
hosts or authentication identities. `ConnectionConfig::new` overrides URL and
validated client name. Explicit duplicate names keep the server's replacement
semantics. TLS is not added; this uses the SDK's native plain-WebSocket profile.

```rust,no_run
use tanuki_ui::app::{App, AppError, ConnectionConfig};

# async fn one_update() -> Result<(), AppError> {
let mut app = App::new(ConnectionConfig::default());
app.reconnect().await?; // Explicit initial connect or reconnect.
let received = app.next_update().await;
// Inspect app.model() or render after a complete batch has applied.
let closed = app.close().await; // Join even when receiving failed.
received?;
closed?;
# Ok(())
# }
```

Reconnect first closes/joins the previous session, then connects once and
registers a raw listener. The terminal calls `begin_reconnect()` to start a
single owned, cancellable connection job; `next_update()` installs its result
without losing it when a render tick cancels a receive poll. No extra update
queue is added. Repeated reconnect keys during a pending attempt are ignored.
Exit cancels and joins a pending job, closing/joining any session it obtained.
The convenience `reconnect().await` remains available to headless callers.
The baseline comes from listener registration, not historical hello. Only successful registration replaces the cache. Replacement
accepts a restarted sequence and preserves surviving selection, expansion and
filter. A failed attempt keeps the previous display and returns a typed error.
`close().await` joins SDK driver/writer tasks; prior abnormal termination may be
returned even though the join completed. Joining the client is not an
acknowledgement of server-side cleanup. Dropping App only signals SDK shutdown;
callers must await close on normal/error exit. Failed setup also closes/joins
any newly created session. After stream termination a session remains owned
until reconnect/exit so it can be joined; no replacement listener is created.

The full `/**` subscription includes currently exposed reserved diagnostics;
real-server tests compare it with an explicitly selected `/$diagnostics/**`
listener. The server currently has no `/$connections/**` projection, so the
inspector cannot display its own session as a node yet. It does not fabricate
connection topics or introspect private state. Schema text says “not available
from this server API”, which does not imply that no schema applies. Schema/link
enumeration and explanation remain later tasks.

Milestone 1 is complete for node inspection. Policy enumeration and explanation,
live policy notifications, occurrence history and complete public-state inventory
are still absent. The next task is checkpoint B (Task 04), a public API design
review before server contract expansion.

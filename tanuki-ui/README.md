# Tanuki inspector foundation

Task 01 implements a pure library (`model`, `format`) and a thin placeholder
binary. It neither connects to a server nor enters terminal mode:

```sh
cargo run -p tanuki-ui
cargo test -p tanuki-ui
```

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

`ConnectionStatus` is display state only at this stage. Task 02 owns SDK
connections and baseline replacement; Task 03 owns terminal layout, input,
scrolling and restoration. Terminal dependencies, controls and smoke checks are
therefore deferred. Schema text says “not available from this server API”,
which does not imply that no schema applies.

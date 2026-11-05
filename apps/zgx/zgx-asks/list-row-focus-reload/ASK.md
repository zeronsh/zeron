# Ask: a reload gives focus back to a `ui.list` row, as it does to any other node

**Priority: developers of any zgx app** (every save reloads; the keyboard drops out of whatever list had it). Seen while making `apps/zgx/tests/reload.rs` `a_reload_keeps_the_panel_where_it_was_scrolled` pass: the file tree's clicked row was focused before the reload and nothing was after.

**Today.** A reload (`ScriptHost::reload`) puts focus back on a plain focusable node that's built again (`ui.row { focusable = true }`). It doesn't put it back on a row of a `ui.list`, even a keyed one whose row is rebuilt under the same key in the same place. Nothing has focus afterwards.

Same on every pin Zeron has used, so this isn't a regression:

| zgx | plain node (before, after) | `ui.list` row (before, after) |
|---|---|---|
| `aaa304eb2` | true, true | true, **false** |
| `03270165c` | true, true | true, **false** |
| `f6506b6e6` | true, true | true, **false** |

**Repro:** `repro/` is a crate of its own (zgx `f6506b6e6`). `ui/app.luau` is a focusable button and a keyed `ui.list` of 50 focusable rows. `cargo test` from `repro/` clicks each, reloads and checks the focus. `a_plain_node_keeps_focus_across_a_reload` passes; `a_list_row_keeps_focus_across_a_reload` fails with `(true, false)`.

**Ask:** match the focused node across a reload the way plain nodes already are, through the list's row key: a list with `key` puts focus back on the row with the same key once it's built (or, if the reload leaves it out of view, once it's built again). A positional list could match by index.

**Meanwhile in Zeron:** `ds/file_tree.luau` takes a `keys` state, the path of the row with the keyboard, which `screens/files/workspace.luau` keeps across reloads. The row with that path focuses itself as it mounts. Drop it once this lands.

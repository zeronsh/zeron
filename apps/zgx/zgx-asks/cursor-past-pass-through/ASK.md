# Ask: the pointer's cursor comes from what takes the pointer

**Who needs it:** everyone using Zeron with a mouse or trackpad. Found Nov 3 2026, after #1194 (cursors gone across the window) and while building Zeron v3's split chats.

**The problem:** `Scene::cursor_at` gives the cursor of the front-most node at the point (`hit_test_all(..).first()`), whether or not that node lets the pointer through (`pointer_events = "none"`). A press there goes past it to what's under it, but the cursor doesn't. So any layer that only frames or positions something (and passes the pointer through) blanks the cursor of everything beneath it:
- the session hover card's window-sized layer hid every cursor (#1194 made it a zero-size anchor);
- the title bar's window-wide strip hid the cursors of the panel's tabs and of a split pane's × along the window's top (Zeron v3 makes the bar a strip of no height its controls hang from);
- the composer's frame over each chat (`screens/chat/composer.luau`'s `ui.overlay`, the whole pane, so it can place the composer by the pane's height) still hides the transcript's text cursor and its links' hand, in a chat alone and in each pane of a split. That one can't be made zero-size without measuring the pane some other way.

**The ask:** `cursor_at` takes the cursor from the node a press would reach: past nodes that pass the pointer through, unless they declare a cursor of their own, as CSS's `pointer-events: none` does for `cursor`.

**Today's workaround:** making each such layer a box of no size (`w = 0, h = 0, overflow = "visible"`) with its content placed from it. `tests/pointer_cursor.rs` holds the ones fixed.

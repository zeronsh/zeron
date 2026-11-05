# Ask: `EditorHandle::look_up` in an editor shown a line at a time

**Priority: medium (worked around in Zeron).** In Zeron's composer, Look Up on a selected word did nothing: `EditorHandle::look_up` returned false and never asked the platform.

**Seen on:** zgx `a2c9cf1` (Zeron's pin), macOS. The same editor in a plain `ui.textarea` looks up fine.

**Why:** `look_up` takes the baseline from `scene.text_baseline(self.text, start)`. The composer's editor is drawn a line at a time (`EditorHint::shown_lines`, the `editor_lines` pool). Its own text node then holds no laid-out text, so `text_baseline` gives `None` and `look_up` returns false. `text_area` already handles this case: it walks `shown_lines()`. `look_up` doesn't.

**Ask:** `look_up` finds the baseline the way `text_area` finds a range: from the shown line node that holds `start`, at that line's offset (`text_baseline(line_node, start - line.start)`), falling back to `self.text`. The same probably applies to anything else that calls `text_baseline(self.text, …)` on an editor.

**Zeron meanwhile:** src/editor_menu.rs `look_up_by_lines`. When `look_up` returns false with text selected, it anchors from `text_area(start..start)` at the line's type size and calls `platform.look_up` itself. Drop it once this lands.

**Repro:** in apps/zgx, the composer fixture with a platform that looks up (the test's `LooksUp`): `editor.look_up(&service)` is `false` on the composer's editor and `true` on a plain `ui.textarea { value = "hello world" }`.

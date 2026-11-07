# imgui-playground

One native window that draws the basic widget set, each call writing straight
into the frame. Its manifest asks for `imgui.windows`.

| What | Value |
| --- | --- |
| Window id | `playground` |
| Window title | `Widget Playground` |
| Requested client size | 560 × 420 |

Widgets, in the order the frame draws them:

| Widget | Call |
| --- | --- |
| Checkbox | `ui.checkbox("watch", watch)` |
| Two radio buttons | `ui.radio_button("left", …)`, `ui.radio_button("right", …)` on one line |
| Float slider | `ui.slider_float("speed", speed, { min = 0, max = 1 })` |
| Integer drag | `ui.drag_int("steps", steps, { min = 0, max = 32 })` |
| Combo | `ui.combo("fruit", fruit, { "apple", "banana", "cherry" })` |
| Three rows | `ui.selectable("row 1" … "row 3", …)` |
| Progress bar | `ui.progress_bar(row / 3)` |
| Bullet line | `ui.bullet_text(…)` |
| Scrolling log | `ui.begin_child("log", { width = 520, height = 96, border = true })` … `ui.end_child()` |
| Values line | one `ui.text` with the current widget values |

The log appends one line per frame and keeps 30, so the child region always
has more than it shows and scrolls.

Run it (the gallery screen needs no engine and no sign-in):

```sh
ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-playground \
ZERON_ZGX_PLUGIN_LOG=1 \
apps/zgx/target/debug/zeron-zgx.exe gallery
```

`zeron.luau` is a byte-for-byte copy of `apps/zgx/plugins-sdk/zeron.luau`;
the plugin uses only that public API.
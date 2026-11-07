# imgui-custom-chrome

A plugin window with no platform title bar that draws its own chrome from
Luau: a title, a drag bar, minimize, maximize, close and a resize probe.

| What | Value |
| --- | --- |
| Window id | `chrome` |
| Window title | `Custom Chrome` |
| Requested client size | 420 × 340 |
| Titlebar | `hidden` |
| Drag | press and move the 32 px bar under the title (`begin_drag`) |
| Double click | click the bar twice within 0.4 s (`maximize` / `restore`) |
| Buttons | `minimize`, `maximize`, `close`, and `resize` (which always refuses: no platform exposes a resize gesture) |
| Every control | prints one `imgui-custom-chrome: …` line, or `… refused: …` |

The bar is an `ui.invisible_button`, so it draws nothing and owns its rect;
its hover and active answers come from `is_item_hovered` / `is_item_active`,
and the drag uses `mouse_delta` / `is_mouse_dragging`.

Run it:

```sh
ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-custom-chrome \
ZERON_ZGX_PLUGIN_LOG=1 \
apps/zgx/target/debug/zeron-zgx.exe gallery
```

`zeron.luau` is a byte-for-byte copy of `apps/zgx/plugins-sdk/zeron.luau`.
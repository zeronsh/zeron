# imgui-system-monitor

A native window that reads like a small system monitor. Its manifest asks for
`imgui.windows`; `init.luau` opens one window and redraws it from Luau every
shown frame.

| What | Value |
| --- | --- |
| Window id | `monitor` |
| Window title | `System Monitor` |
| Requested client size | 520 × 380 |
| Header | `System Monitor` |
| Readings | `cpu N%` in `#8fd694`, `memory N%` in `#ffb454` |
| Bars | one `progress_bar` per reading, fed by the frame counter |
| Button | `Pause` / `Resume` (a `small_button`) toggles sampling |
| On close | prints one line: `imgui-system-monitor: closed after N frames` |

Both bars move while sampling runs. `Pause` stops the counter, so the bars
hold where they were and the button reads `Resume`.

Run it (the gallery screen needs no engine and no sign-in):

```sh
ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-system-monitor \
ZERON_ZGX_PLUGIN_LOG=1 \
apps/zgx/target/debug/zeron-zgx.exe gallery
```

`zeron.luau` is a byte-for-byte copy of `apps/zgx/plugins-sdk/zeron.luau`;
the plugin uses only that public API.
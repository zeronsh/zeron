# imgui-win-probe-broken-open

The validation contract's **failure fixture** for a window whose first frame
cannot be drawn. It opens one window and its *first* `on_render` raises
`error("probe open failure")`; a host that swallows that frame draws the line
`probe: the first frame raised probe open failure` instead, so a swallowed
failure is visible rather than an empty window.

Development fixture only (`plugins-dev`). `zeron.luau` is a byte-for-byte copy
of `apps/zgx/plugins-sdk/zeron.luau`.

## What it serves

`VAL-LIFE-012`: once the first-frame failure is reported, no native window for
this plugin may exist (the process's top-level window count returns to its
pre-enable value), the plugin log carries the error sentence naming
`probe open failure`, the window is not recreated, and Zeron stays up with its
main window answering.

On the **dev route** (`ZERON_ZGX_DEV_PLUGINS`) there is no plugin manager, so
the plugin itself stays loaded and its tool still answers; only the window
goes. On the **installed route** the plugin is additionally marked broken and
withdrawn, so its row in Settings → Plugins carries `probe open failure` and
its `probe_status` tool stops answering.

## Run it

```sh
ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-win-probe-broken-open \
ZERON_ZGX_PLUGIN_LOG=1 \
apps/zgx/target/debug/zeron-zgx.exe gallery
```

## Its window and tools

| id | title | size |
| --- | --- | --- |
| `broken` | `Probe Broken Open` | 420 × 300 |

* `probe_status` — answers only while the plugin is loaded; a broken plugin's
  tool must not answer.
* A menu item (`Probe: report the open failure`) exists so a person can see
  whether the plugin is still around.

## Plugin log

`probe-broken: opened broken: the first render raises probe open failure`, then
the host's own failure report
(`Window "broken"'s first frame failed: … probe open failure`), then
`probe-broken: closed:closecall broken` because the host's own close reaches
`on_close`.

## Tests

`cargo test --bin zeron-zgx fixtures_tests` loads the folder, pushes the
window's first frame, and asserts the frame ends with `ok = false` and the
error `probe open failure` — and that nothing was drawn.

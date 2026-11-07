# imgui-win-probe-broken-open

The validation contract's **failure fixture** for a window whose first frame
cannot be drawn. It opens one window and its *first* `on_render` raises
`error("probe open failure")`; a host that swallows that frame draws the line
`probe: the first frame raised probe open failure` instead, so a swallowed
failure is visible rather than an empty window.

Development fixture only (`plugins-dev`). `zeron.luau` is a byte-for-byte copy
of `apps/zgx/plugins-sdk/zeron.luau`.

## What it serves

`VAL-LIFE-007`: once the failure is reported, no native window for this plugin
may exist (the process's top-level window count returns to its pre-enable
value), the plugin's row shows `probe open failure`, and the plugin is not
loaded (its `probe_status` tool does not answer). If the host instead leaves a
window behind or keeps the plugin loaded, that is the defect this fixture
exposes.

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
the host's own failure report, then `probe-broken: closed:closecall broken`
if the close reaches `on_close`.

## Tests

`cargo test --bin zeron-zgx fixtures_tests` loads the folder, pushes the
window's first frame, and asserts the frame ends with `ok = false` and the
error `probe open failure` — and that nothing was drawn.

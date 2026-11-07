# imgui-win-probe-legacy

`VAL-WIN-010(b)`: the copy-edited-manifest bypass regression. Its manifest
asks for the existing **`windows`** permission — the one that opens Zeron's
declarative `ui.window` — and not `imgui.windows`. The declarative panel keeps
its old meaning; the native request is refused with a sentence naming
`imgui.windows`, and the plugin stays loaded.

Development fixture only (`plugins-dev`). `zeron.luau` is a byte-for-byte copy
of `apps/zgx/plugins-sdk/zeron.luau`.

## Run it

```sh
ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-win-probe-legacy \
ZERON_ZGX_PLUGIN_LOG=1 \
apps/zgx/target/debug/zeron-zgx.exe gallery
```

## What it declares and does

* `ui.window { id = "legacy-panel", open = shown, node = … }` — a declarative
  window, the capability `windows` has always granted.
* `pcall(zeron.imgui.open_window { id = "x", title = "Denied" })` — refused;
  the sentence is printed as
  `probe-legacy: imgui refused=<true|false> error=<sentence>`.
* `probe-legacy: alive <n>` once a second, so the plugin is plainly still on.

## Tests

`cargo test --bin zeron-zgx fixtures_tests` loads the folder and asserts the
manifest asks for `windows` only, the declarative window is declared, the
native request is refused and handled, and the plugin keeps running.

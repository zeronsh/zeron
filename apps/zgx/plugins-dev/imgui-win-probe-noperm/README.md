# imgui-win-probe-noperm

`VAL-WIN-010(a)`: a fixture whose manifest declares **no permissions at all**.
It asks for one native window, is refused with a sentence naming
`imgui.windows`, logs that sentence, and keeps running (a refused permission is
not a crash). No window may appear and the plugin may not be withdrawn.

Development fixture only (`plugins-dev`). `zeron.luau` is a byte-for-byte copy
of `apps/zgx/plugins-sdk/zeron.luau`.

## Run it

```sh
ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-win-probe-noperm \
ZERON_ZGX_PLUGIN_LOG=1 \
apps/zgx/target/debug/zeron-zgx.exe gallery
```

It asks once for `{ id = "x", title = "Denied" }` inside `pcall` and prints
`probe-noperm: imgui refused=<true|false> error=<sentence>`, then prints
`probe-noperm: alive <n>` once a second — the proof it was not withdrawn.

## Tests

`cargo test --bin zeron-zgx fixtures_tests` loads the folder and asserts the
manifest asks for nothing, one `open_window` request is made, the host's
refusal is handled (no script error), and the plugin keeps running.

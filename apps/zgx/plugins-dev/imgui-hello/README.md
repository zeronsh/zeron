# imgui-hello

The smallest plugin with a native Dear ImGui window. Its manifest asks for
`imgui.windows`; `init.luau` opens one window and draws it from Luau.

| What | Value |
| --- | --- |
| Window id | `hello` |
| Window title | `Hello from Luau` |
| Requested client size | 520 × 300 |
| Drawn every frame | `Hello from Luau! This window is drawn by a plugin.`, `frame N`, the `Click me` button, `clicks N` |
| On a button release | prints exactly one line: `imgui-hello: clicked N` |

`frame N` counts the render callbacks the host has run for the window. It
advances while the window is shown and stops while it is hidden or
minimized, because no callback runs then.

Run it (the gallery screen needs no engine and no sign-in):

```sh
ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-hello \
ZERON_ZGX_PLUGIN_LOG=1 \
apps/zgx/target/debug/zeron-zgx.exe gallery
```

`zeron.luau` is a byte-for-byte copy of `apps/zgx/plugins-sdk/zeron.luau`;
the plugin uses only that public API. `cargo test --bin zeron-zgx
hello_tests` loads this folder as the bridge loads a snapshot and drives
its frames headlessly.

# imgui-developer-console

A native window shaped like a console. Its manifest asks for `imgui.windows`;
`init.luau` opens one window with a command line, a log and a Run button.

| What | Value |
| --- | --- |
| Window id | `console` |
| Window title | `Developer Console` |
| Requested client size | 640 × 420 |
| Command line | `ui.input_text` labelled `command`, with a `type a command` hint |
| Log | the last 20 lines, drawn with `ui.text`, oldest at the top |
| Button | `Run` appends `echo <command>` and clears the line |
| Enter | not wired: `input_text` gives back the text, not a key press |

The log keeps 20 lines; each new line drops the oldest. A `Run` with an empty
command adds nothing.

Run it (the gallery screen needs no engine and no sign-in):

```sh
ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-developer-console \
ZERON_ZGX_PLUGIN_LOG=1 \
apps/zgx/target/debug/zeron-zgx.exe gallery
```

`zeron.luau` is a byte-for-byte copy of `apps/zgx/plugins-sdk/zeron.luau`;
the plugin uses only that public API.
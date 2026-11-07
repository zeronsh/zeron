# imgui-no-permission

The validation contract's **consent fixture**. Its manifest asks for no
`imgui.windows`. It tries to open one window, is refused with a sentence
naming the permission, and keeps asking twice a second — so the consent row can
add `imgui.windows` to this manifest, install the plugin, allow it in the
consent sheet, and see the retried call succeed within 120 frames without a
restart or a manual reload.

Development fixture only (`plugins-dev`). `zeron.luau` is a byte-for-byte copy
of `apps/zgx/plugins-sdk/zeron.luau`.

## What it serves

* `VAL-WIN-010`: its `open_window` fails with a message naming `imgui.windows`
  and saying it can be asked for in Zeron; no window appears; the plugin stays
  loaded and enabled.
* The consent row: with `imgui.windows` added to `zeron-plugin.json` and
  allowed, exactly one window titled `No Permission Probe` appears at 480 × 320
  and the record reads on. Consent is the only action needed.

## Run it

```sh
ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-no-permission \
ZERON_ZGX_PLUGIN_LOG=1 \
apps/zgx/target/debug/zeron-zgx.exe gallery
```

## Its window

| id | title | size |
| --- | --- | --- |
| `consent` | `No Permission Probe` | 480 × 320 |

Opened only after a retry is allowed. It draws its title, the number of asks
and a `Ping` button.

## Tools and menu

* `probe_status` — `{ plugin, version, windows, open, asked, last_refusal }`:
  how many asks were refused, the exact sentence, and whether the window is
  open.
* Menu item `Probe: ask for imgui.windows again` — asks at once instead of
  waiting for the timer.

## Plugin log

`probe-no-permission: refused: <sentence>` (first ask, then every second),
`probe-no-permission: opened after <n> asks`, and
`probe-no-permission: closed:closecall consent`.

## Tests

`cargo test --bin zeron-zgx fixtures_tests` loads the folder with a fake host
that refuses the first asks, then accepts: it asserts the manifest declares no
`imgui.windows`, that the fixture asks again after each refusal, and that it
opens and draws once allowed.

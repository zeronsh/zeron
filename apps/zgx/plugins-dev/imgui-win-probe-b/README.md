# imgui-win-probe-b

The validation contract's **second plugin**. It opens a window with the same
id the main probe uses, `inspector`, at its own size and position, so the
rows that scope a window id to its plugin can show that two plugins' `inspector`
windows are two different native windows.

Development fixture only (`plugins-dev`). `zeron.luau` is a byte-for-byte copy
of `apps/zgx/plugins-sdk/zeron.luau`; only the public API is used.

## Run it with the main probe

```sh
ZERON_ZGX_DEV_PLUGINS="apps/zgx/plugins-dev/imgui-win-probe:apps/zgx/plugins-dev/imgui-win-probe-b" \
ZERON_ZGX_PLUGIN_LOG=1 \
apps/zgx/target/debug/zeron-zgx.exe gallery
```

(`:` separates the folders on macOS/Linux; use `;` on Windows, or two entries
in `ZERON_ZGX_DEV_PLUGINS`.)

## Its window

| id | title | size | options |
| --- | --- | --- | --- |
| `inspector` | `B Inspector` | 700 × 500 | at (500, 300), `remember = true` |

The main probe's `inspector` is `Probe Inspector`, 720 × 480 at (120, 80): the
same id, a different plugin, and geometry neither can share. `remember = true`
lets each plugin's inspector come back at its own size after a restart
(`VAL-LIFE-009`).

It draws `window inspector`, `plugin B`, `frame <n>`, `clicks <n>` and a
`Ping` button; `frame` is the count of render callbacks, so a probe can see B
keeps rendering while A's window closes (`VAL-WIN-006`).

## Probe actions

Menu items (`Probe B: …`) and `probe_run { action = "…" }` calls:

| action | what it does | rows |
| --- | --- | --- |
| `close_inspector` | `inspector:close()` | VAL-WIN-006 |
| `reopen` | closes, then reopens `inspector` after 300 ms | VAL-WIN-006, VAL-LIFE-005 |
| `retitle_inspector` | `set_title("Inspector")`, so no title distinguishes the two windows | VAL-WIN-006 second run |
| `retitle_b` | `set_title("B Inspector")` | VAL-WIN-006 |
| `resize_1100` | `set_size(1100 × 800)` | VAL-LIFE-009 |
| `bump` | increments B's counter | VAL-CROSS-003 |

## Tools

* `probe_status` — `{ plugin, version, windows, open, frames, counter, closed,
  last_action }`; answers while its window is closed.
* `probe_run` — runs one action above.

## Plugin log

`probe-b: opened:inspector:B Inspector`, `probe-b: closed:closecall <id>`
(exactly one per close), `probe-b: clicked:<n>`, and an `open refused:` line
if the host refuses the window.

## Tests

`cargo test --bin zeron-zgx fixtures_tests` loads this folder like the bridge
does and asserts the manifest, the `inspector` window's spec (distinct from the
main probe's), its actions and its tool.

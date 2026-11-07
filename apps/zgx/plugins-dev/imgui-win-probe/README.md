# imgui-win-probe

The validation contract's **main probe fixture**. It opens native Dear ImGui
windows with the ids, titles, sizes, positions and options the window,
lifecycle, input and docking rows name, and it exposes probe actions, a
`probe_status` tool, a timer and store keys so those rows can be driven and
observed.

It is a development fixture (`plugins-dev`, never `plugins-builtin`) and uses
only the public `zeron` SDK. `zeron.luau` here is a byte-for-byte copy of
`apps/zgx/plugins-sdk/zeron.luau`.

## Run it

The gallery screen needs no engine and no sign-in:

```sh
ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-win-probe \
ZERON_ZGX_PLUGIN_LOG=1 \
apps/zgx/target/debug/zeron-zgx.exe gallery
```

On Windows PowerShell:

```powershell
cd apps/zgx
$env:ZERON_ZGX_DEV_PLUGINS = (Resolve-Path .\plugins-dev\imgui-win-probe).Path
$env:ZERON_ZGX_PLUGIN_LOG = "1"
.\target\debug\zeron-zgx.exe gallery
```

## Windows it opens at launch

The host allows **8 native windows per plugin** (validation contract §5). Four
open at launch, leaving four slots free for the one-action-away windows. They
are the ones a row needs *before* a probe action runs.

| id | title | size | options |
| --- | --- | --- | --- |
| `a` | `Probe A` | 520 × 360 | |
| `b` | `Probe B` | 420 × 300 | |
| `inspector` | `Probe Inspector` | 720 × 480 | at (120, 80), `remember = true` |
| `bare` | `Probe Bare` | 600 × 400 | `titlebar = "hidden"`, `decorations = false` |

Nothing else opens until an action asks for it.

## Windows one action away

Each action opens with a plain `open_window` and no close first: the four free
slots absorb it, and two-window actions still fit under the live cap. Every
action is an item in Zeron's one `Probe Actions` menu bar menu, titled by the
action's own name, and is also callable by name through the `probe_run` tool.

| id | title | size | options | action |
| --- | --- | --- | --- | --- |
| `bounds` | `Probe Bounds` | 600 × 400 | `min_size` 400 × 300, `max_size` 900 × 600, resizable | `open_bounds` |
| `drag` | `Probe Drag` | 640 × 420 | `titlebar = "hidden"`, `decorations = false`, resizable | `open_drag` |
| `size` | `Probe Size` | 720 × 480 | resizable | `open_size` |
| `pos` | `Probe Pos` | 400 × 300 | at (100, 120) | `open_pos` |
| `pos2` | `Probe Pos Two` | 400 × 300 | at (600, 420) | `open_pos2` |
| `quiet` | `Probe Quiet` | 480 × 360 | `visible` from `imgui.windows.quiet.visible` (false) | `open_quiet` |
| `left` | `Probe Left` | 500 × 400 | `remember = true` | `open_left`, `open_both_sides` |
| `right` | `Probe Right` | 900 × 700 | `remember = true` | `open_right`, `open_both_sides` |
| `dup` | `Probe Dup` | 380 × 280 | | `open_dup` (then a second `open_window` for `dup`) |
| `reuse` | `Probe Reuse` | 400 × 300 | | `open_reuse`, `reuse_cycle` |
| `n1`…`n8` | `Probe N1`…`Probe N8` | growing | | `n_loop`, then `n_more` |

`remember = true` is set only where a row measures restored geometry: the
inspector (`VAL-LIFE-007`, `VAL-LIFE-009`), and left/right (`VAL-LIFE-009`).
Every other window opens at the size its request names.

## Probe actions

All of these are items in the `Probe Actions` menu (each titled by its action
name) and `probe_run { action = "…" }` calls. The tool returns `{ action, ok }`;
an unknown name raises a sentence naming it.

| action | what it does | rows |
| --- | --- | --- |
| `reopen` | closes `inspector`, then reopens it by the same id with the same options after 300 ms | VAL-LIFE-007, VAL-DOCK-006 |
| `blow_up` | the next `on_render` raises `error("probe render failure")` | VAL-LIFE-011 |
| `close_inspector` | `inspector:close()` | VAL-WIN-006 |
| `inspector_as_a` | `set_title("A Inspector")`, `set_size(400 × 300)` | VAL-WIN-006 |
| `inspector_unnamed` | `set_title("Inspector")` | VAL-WIN-006 second run |
| `inspector_700` | `set_size(700 × 500)` | VAL-LIFE-009 |
| `rename_inspector` | `set_title("Renamed Probe")` | VAL-WIN-012 |
| `resize_inspector` | `set_size(1000 × 640)` | VAL-WIN-012 |
| `bump_inspector` | increments the inspector's own counter | VAL-WIN-016 |
| `hide_inspector` / `show_inspector` / `focus_inspector` / `minimize_inspector` / `restore_inspector` | the five window controls | VAL-WIN-011, VAL-WIN-017 |
| `open_bounds` | opens `bounds` | VAL-WIN-004 |
| `open_drag` | opens `drag` | VAL-WIN-019…022 |
| `open_size` | opens `size` (720 × 480, resizable) | VAL-WIN-002 |
| `open_pos` | opens `pos` at (100, 120) | VAL-WIN-003 |
| `open_pos2` | opens `pos2` at (600, 420) | VAL-WIN-003 |
| `open_quiet` | opens `quiet` hidden | VAL-LIFE-016 |
| `open_left` / `open_right` / `open_both_sides` | opens the remembered side windows | VAL-LIFE-009, VAL-DOCK-003…005 |
| `open_dup` | opens `dup`, then attempts `dup` again and records the refusal | VAL-WIN-007 |
| `open_reuse` / `reuse_cycle` | opens, closes and reopens `reuse` (title `Probe Reuse Again`) | VAL-WIN-008 |
| `n_loop` | closes every window, then opens `n1`… until the host refuses | VAL-WIN-009 |
| `n_more` | closes the highest `n` and opens the next | VAL-WIN-009 |
| `close_all_windows` | closes every window; the plugin stays loaded | VAL-WIN-010 |
| `open_a` / `open_b` / `open_c` | open a probe window (`a`, `b` or `c`) after a close | VAL-WIN-005, VAL-WIN-016 |

## Tools

* `probe_status` — its own JSON, readable while no window is open:
  `{ plugin, version, windows, open, frames, counters, closed, ticks,
  last_action, last_refusal, store = { quiet_visible, inspector_prefs,
  drag_prefs } }`. `frames`, `counters` and `closed` are keyed by window id.
* `probe_run` — `{ action = "<name>" }` runs one probe action from the table
  above.

Both are listed in `zeron-plugin.json`, so a probe reaches them through
Zeron's MCP server, and they keep answering after every window is closed
(`VAL-LIFE-007`).

## What each window draws

`window <id>`, `open <true|false>`, `frame <n>`, `clicks <n>`, `tick <n>` and a
`Ping` button that increments that window's own counter. The counter is keyed
by window id, never shared, so a click in one window cannot change another
(`VAL-WIN-016`). The frame count advances once per render callback and stops
while the window is hidden or minimized (`VAL-WIN-017`).

Only `ui.text` and `ui.button` exist in the host today; the single-line text
fields the input rows name, and the plugin-drawn drag gesture on `drag`, arrive
with the `basic-widgets`, `input-fidelity` and `custom-chrome-sdk` features.
The probe action table is where they attach; the window set and options here
do not change.

## Timer and store

The timer (`ui.every(250 ms)`) increments `ticks` even while no window is
open; `probe_status.ticks` is the value the lifecycle rows watch.

Store keys, all under `imgui.windows.<window id>.*`:

| key | value |
| --- | --- |
| `imgui.windows.quiet.visible` | `false` — read at launch and passed to `quiet` when it opens |
| `imgui.windows.inspector.prefs` | `{ title = "Probe Inspector", size = { width = 720, height = 480 } }` |
| `imgui.windows.drag.prefs` | `{ bar = 32 }` |

## Plugin log

With `ZERON_ZGX_PLUGIN_LOG=1` the fixture prints, each line starting `probe:`:

* `probe: ready windows=a,b,bare,inspector ticks=0`
* `probe: opened:<id>:<title>` and `probe: open:<id> refused: <sentence>`
* `probe: closed:closecall <id>` — exactly one per close (the line
  `VAL-WIN-013` counts), from the plugin's own `close()` or the OS chrome
* `probe: clicked:<id>:<n>`
* `probe: tick <n>` every 20 ticks
* `probe: dup refused=<true|false> error=<sentence>`
* `probe: limit stopped at <n>: <sentence>`

## The cap, and why not all rows' windows at once

The contract names windows for more simultaneous rows than the four launch
windows, while the canonical limit is 8 per plugin. The four above are the
ones rows need before any action runs; the rest are one `probe_run`/menu
action away and each opens with no close first, leaving four slots free for
them. The fixture logs the host's refusal, naming the limit, whenever a
request exceeds it.

## Tests

`cargo test --bin zeron-zgx fixtures_tests` loads this folder exactly as the
bridge loads a snapshot (its `init.luau` beside its own SDK copy) with a fake
host, and asserts its manifest, the four launch windows' ids/titles/sizes/
options, that no other window opens unasked, every probe action, the
`probe_status`/`probe_run` tools, the per-window counters, the timer, the
store keys and the single `closed:closecall` log path.

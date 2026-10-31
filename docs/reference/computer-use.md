# Computer use (cua-driver)

Zeron drives the Mac through a vendored [cua-driver](https://cua.ai/docs/cua-driver)
runtime. `zeron mcp` proxies its tools to the agent as `computer_*`, so the agent
can read accessibility trees, click, type, and drive browsers in the background
without taking focus.

## Runtime

The runtime is the **whole `CuaDriver.app` bundle**, not a bare binary. Upstream
builds since 0.34 are killed on launch outside their bundle because the daemon
needs its own bundle identity for a WindowServer connection. Layout:

```
vendor/cua-driver/
├── CuaDriver.app/                                # stock upstream runtime (0.36.0)
└── cursor-themes/sh.zeron.cursor.aurora.cua-theme
```

`scripts/provision-cua-driver.sh` stages it from `/Applications/CuaDriver.app`
(or `CUA_DRIVER_SOURCE`); the packaging scripts copy it to
`Zeron.app/Contents/Resources/cua-driver/`. `locate_driver()` looks there first,
then at `/Applications/CuaDriver.app`, then `PATH`; `ZERON_CUA_DRIVER` overrides
everything.

The vendored build is **stock upstream**, not the patched Synara 0.28.2 binary
Zeron used before. Two consequences:

- `--compact-cursor` and `set_agent_cursor_style` (fill/rim colors) no longer
  exist. Cursor appearance is a compiled theme, and badge detail is the driver's
  `cursor.badges` config value (`minimal` is the upstream default).
- Features upstream shipped since 0.28 are available: background drag and menu
  commands, background typing into LibreOffice, reading a page on another Space,
  background session cursors, and `start_session`-scoped identity.

## Agent cursor theme

`Aurora` is Zeron's own cursor theme: a glass pointer with an aurora
violet→cyan bloom, authored to the `cua-driver-actions-v2` profile (twelve
semantic actions, 128×128 at 30 fps, static path geometry).

- Source generator: `assets/cursor-themes/aurora.py`
- Checked-in artifacts: `assets/cursor-themes/aurora.lottie` (dotLottie source)
  and `aurora.cua-theme` (compiled, what the driver loads)

The bridge points the daemon at the theme directory with
`CUA_DRIVER_CURSOR_THEME_DIR`, so the theme ships with the app and never touches
the user's shared `~/Library/Application Support/Cua Driver/cursor-themes`.
Filename must match the theme id (`sh.zeron.cursor.aurora.cua-theme`).

Rebuild after editing the generator:

```bash
python3 assets/cursor-themes/aurora.py --output /tmp/aurora.lottie
C=/Applications/CuaDriver.app/Contents/MacOS/cua-cursor-theme
$C validate /tmp/aurora.lottie
$C build /tmp/aurora.lottie --output assets/cursor-themes/aurora.cua-theme
$C preview assets/cursor-themes/aurora.cua-theme --output /tmp/preview   # still frames, for review
```

`cursor-theme build` needs the authoring sidecar, which lives beside the driver
binary inside the bundle. Running `cua-driver` through a symlink in `~/.local/bin`
makes the compiler lookup fail; call the bundle path directly, or symlink
`cua-cursor-theme` next to it too.

Users pick a theme in Settings → Computer use. The saved setting is a theme id in
`computer-use-cursor.json`; `Stock` means the driver's `cua.default`.

## Instructions the agent receives

- `DRIVER_GUIDANCE` — the always-on server instructions (targeted reads,
  `element_token` = `<snapshot_id>:N`, `since:"latest"` diffs, background-first).
- `computer_help {topic}` — on-demand playbooks: `browser`, `batching`, `menus`,
  `forms`, `recovery`, `editors`, `electron`, `terminals`, `finder`.

`run_steps` and `run_script` dispatch real input, so they are in `ACTION_TOOLS`:
Escape and Stop interrupt them like any other action.

## The agent cursor, never the user's mouse

Zeron's computer use is background-only. The pointer tools (`click`,
`double_click`, `right_click`, `drag`, `scroll`, `move_cursor`) stay available,
but `pointer_refusal()` in `crates/mcp/src/computer.rs` rejects any call that
would reach the user's physical pointer before it is dispatched:

- `scope: "desktop"` — acts in true screen pixels instead of window pixels;
- `delivery_mode: "foreground"` — drives the physical cursor through the
  gesture.

The same check runs over every `run_steps` entry, in both the `{tool, args}` and
`{<tool>: args}` forms, and `foreground_fallback` is refused because the driver
retries a refused step in the foreground with it. A script's own calls happen
inside the driver, so `run_script` is guarded coarsely by name: a script whose
text contains `desktop` or `foreground` is refused before it runs. A refused
call returns `computer_pointer_forbidden` (layer `zeron-host`) and the driver
never sees it, so what the agent does on screen is always the agent cursor
showing a background accessibility action.

## Interrupting the agent (Escape, Stop, turn end)

Upstream 0.36 has **no interrupt method** and does not watch for a physical
Escape: the patched Synara `interrupt_input` (with its admission epoch) is gone,
and unknown socket methods are refused outright. Input safety is therefore the
bridge's own, in `crates/mcp/src/computer.rs`:

1. The proxy ends its two daemon sessions (`end_session` on the desktop and
   browser labels). An ended session refuses every later call, so input queued
   for it cannot land; the next call revives the label through `start_session`,
   which re-applies the cursor theme and motion.
2. It bumps a local interrupt epoch. An action whose reply arrives after the
   epoch moved is reported as stopped with an uncertain effect, never as a
   success the model may build on.
3. The admission gate refuses new input until a fresh observation
   (`computer_get_window_state` / `computer_get_browser_state`) reopens it; a
   physical Escape also holds a 1.5 s cooldown.

`end_session` does not abort an already dispatched action; it waits for it. The
proxy stops waiting immediately and returns the uncertain verdict, so what the
user sees is responsive even while the driver finishes in the background.

## Permissions

macOS grants Accessibility, Screen Recording and Input Monitoring to the
**Zeron** app (and, for a standalone runtime, to `CuaDriver`). Dev builds need a
stable signing identity so grants survive rebuilds:
`scripts/provision-local-signing.sh`.

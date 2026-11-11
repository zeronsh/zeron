#!/usr/bin/env python3
"""Make the scratch ui/ the native reference is drawn from.

Copies apps/zgx/ui to OUT and adds `landing_hero_fixture.luau`: the main
window's shell (ui/shell_fixture.luau, the app's own stubs) as a first-time
user sees a new chat: one device, one project, no chats; dark, opaque; the
Parallax scenes plugin's Blossom scene as the wallpaper under the ASCII effect.

A release zeron-zgx reads no ZERON_ZGX_DEV_PLUGINS, so the plugin's scene is
built in the background place by the plugin's own `Scene` code (copied from
plugins-builtin/parallax-scenes/init.luau verbatim but for the art paths and
the pointer, read from the page's own states, as `zeron.live("pointer")`
reads them through the place's `provide`).

usage: make-ui.py <repo> <out>
"""
import re
import shutil
import sys
from pathlib import Path

repo, out = Path(sys.argv[1]), Path(sys.argv[2])
zgx = repo / "apps/zgx"
if out.exists():
    shutil.rmtree(out)
shutil.copytree(zgx / "ui", out)
art = (zgx / "plugins-builtin/parallax-scenes/art").resolve()

# --- the fixture: shell_fixture.luau with a first-time user's data ---------
src = (out / "shell_fixture.luau").read_text()
start = src.index("local model = {")
end = src.index("\n}\n", start) + 3
model = '''local model = {
	here = "mac",
	devices = {
		{ id = "mac", name = "MacBook Pro", platform = "macos", here = true },
	},
	projects = {
		{ id = "hello", name = "hello-world", device = "mac", path = "/Users/you/hello-world", git = true },
	},
	chats = {} :: { any },
	archived = {} :: { any },
	places = { pinned = {}, sections = {} } :: any,
}
'''
src = src[:start] + model + src[end:]
src = src.replace('appearance.wallpaper:set("none")', "-- (landing: the plugin's wallpaper, below)")
src = src.replace('require("@app/screens/shell/sidebar").options.organize:set("project")', "")
tail = '''
-- landing: dark, opaque, the Parallax scenes plugin used as the wallpaper
-- under Zeron's ASCII effect, its scene Blossom.
ui.theme.follow_system(false)
ui.theme.current:set("zeron-dark")
require("@ds/material").frosted:set(false)
ui.shared("plugins", nil :: any):set({
	state = "ready",
	list = { { id = "parallax-scenes", name = "Parallax scenes", status = "on", ui = { change = { "#new-chat-background" } }, permissions = { "ui.change" }, allowed = { "ui.change" }, ask = {} } },
} :: any)
appearance.plugin:set("parallax-scenes")
appearance.plugin_effect:set("ascii")
'''
src = src.replace("local shell = require(\"@app/screens/shell\")", tail + "local shell = require(\"@app/screens/shell\")")
(out / "landing_hero_fixture.luau").write_text(src)

# --- the place: the plugin's scene where it would replace the box ----------
page = out / "screens/shell/page.luau"
p = page.read_text()
needle = '''				-- Where a plugin draws: nothing of Zeron's own.
				return ui.div { id = "new-chat-background", absolute = true, left = 0, right = 0, top = 0, bottom = 0 }'''
assert needle in p, "page.luau's background place moved"
scene = f'''				-- (landing: the Parallax scenes plugin's Scene, as its ui.change
				-- replaces this place; plugins-builtin/parallax-scenes/init.luau.)
				local STEP_X, STEP_Y = 14, 9
				local SPRING = {{ stiffness = 140, damping = 20 }}
				local reach = 1
				local names = {{ "sky", "far", "near", "detail" }}
				local deepest = #names - 1
				local over_x, over_y = math.ceil(deepest * STEP_X * reach), math.ceil(deepest * STEP_Y * reach)
				local layers = {{}}
				for i, layer in names do
					local d = (i - 1) * reach
					table.insert(layers, ui.div {{
						absolute = true, left = -over_x, right = -over_x, top = -over_y, bottom = -over_y,
						x = if d > 0 then ui.spring(px, {{ from = d * STEP_X, to = -d * STEP_X, stiffness = SPRING.stiffness, damping = SPRING.damping }}) else nil,
						y = if d > 0 then ui.spring(py, {{ from = d * STEP_Y, to = -d * STEP_Y, stiffness = SPRING.stiffness, damping = SPRING.damping }}) else nil,
						ui.image {{ src = "{art}/blossom/" .. layer .. ".webp", w_full = true, h_full = true, fit = "cover", aria_hidden = true }},
					}})
				end
				return ui.div {{ id = "new-chat-background", absolute = true, left = 0, right = 0, top = 0, bottom = 0,
					ui.div {{ id = "parallax-scene", class = "scene-blossom", w_full = true, h_full = true, relative = true, overflow = "hidden", role = "img", table.unpack(layers) }} }}'''
p = p.replace(needle, scene)
page.write_text(p)
print(out / "landing_hero_fixture.luau")

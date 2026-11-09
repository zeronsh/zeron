-- A reply's words drawn by one of its two renderers, for tests/prose_pixels:
-- ds/markdown (`pixels.renderer` "luau") or zgx's markdown view with
-- ds/prose's style ("native"), in the window's theme (`pixels.theme`, its
-- dark or light scheme) as a transcript shows it. `pixels.text` is the
-- reply's text; `pixels.key` names it while it streams in (a chunk at a
-- time, parsed for what's new), "" once it's whole. `pixels.veil`: its
-- words fade in as they arrive (with a key), `pixels.seed` what's there as
-- it's made showing at once.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Markdown = require("@ds/markdown")
local Prose = require("@ds/prose")
local look = require("@ds/look")

local text = ui.shared("pixels.text", "")
local renderer = ui.shared("pixels.renderer", "luau")
local key = ui.shared("pixels.key", "")
local scheme = ui.shared("pixels.theme", "dark")
local veil = ui.shared("pixels.veil", false)
local seed = ui.shared("pixels.seed", false)
local veiled = veil:peek() and key:peek() ~= ""
-- What the page knows of the reply's links and carries, as a transcript's
-- (`pixels.known`: a skill the chat learns of after the reply shows).
local known = ui.shared("pixels.known", false)
local function link_chip(target: string): { file: boolean?, skill: string? }?
	if target == "src/config.rs:12" then
		return { file = true }
	elseif target == "my-skill" and known:get() then
		return { skill = "my-skill" }
	end
	return nil
end
local attached = { ["plan.md"] = { kind = "file" }, ["screen"] = { kind = "image" }, ["the thread"] = { kind = "chat" } }

ui.theme.current:set(if scheme:peek() == "light" then "theme-light" else "theme-dark")

local function words(): string
	return text:get()
end

local reply: Node
-- (`native-bold-700`: the native view with bold a step off, which the test
-- must tell apart.)
local style = Prose.style()
if renderer:peek() == "native-bold-700" then
	style.strong = { weight = 700 }
end
if renderer:peek() ~= "luau" then
	reply = ui.column {
		w_full = true,
		min_w = 0,
		ui.markdown_view {
			Prose.prepared(words),
			style = style,
			on_link = Prose.on_link(nil),
			link_menu = Prose.link_menu(nil, nil),
			-- (Nothing opens a path here: only web addresses are links.)
			links = "web",
			autolink = true,
			veil = if veiled then { key = key:peek(), seed = seed:peek() } else nil,
			chip = Prose.chip { link_chip = link_chip, attached = attached },
			attached = Prose.attached(attached),
			w_full = true,
			min_w = 0,
		},
	}
else
	reply = Markdown {
		text = words,
		key = if key:peek() ~= "" then key:peek() else nil,
		veil = veiled,
		veil_seed = seed:peek(),
		link_chip = link_chip,
		attached = attached,
	}
end

return ui.column {
	id = "page",
	w_full = true,
	h_full = true,
	p = "$space.6",
	-- (The chat page's card, under a transcript's words.)
	bg = "$zeron.inset.bg",
	-- The window's own over its theme (ds/look: accent throughout, text
	-- size, fonts), as the main window wears it.
	theme = look.window_theme(),
	selectable = true,
	ui.column { id = "reply", w_full = true, min_w = 0, reply },
}

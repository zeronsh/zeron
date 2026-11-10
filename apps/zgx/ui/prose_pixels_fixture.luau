-- A reply's words drawn by one of its two renderers, for tests/prose_pixels:
-- ds/markdown (`pixels.renderer` "luau") or zgx's markdown view with
-- ds/prose's style ("native"), in the window's theme (`pixels.theme`, its
-- dark or light scheme) as a transcript shows it. `pixels.text` is the
-- reply's text; `pixels.key` names it while it streams in (a chunk at a
-- time, parsed for what's new), "" once it's whole. `pixels.veil`: its
-- words fade in as they arrive (with a key), `pixels.seed` what's there as
-- it's made showing at once. `pixels.live`: the native reply reads its
-- words from `pixels.words` (its text, its math read into code by the
-- test, grown a chunk at a time) as a transcript's reply being written
-- reads the host's, and its text again once that's false.

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
local live = ui.shared("pixels.live", false)
local live_words = ui.shared("pixels.words", "")
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
-- A find over the reply (`pixels.finding`): its query and where its
-- current match is (`pixels.at`: { query, block, k }, one write as a find
-- bar makes it), each block it reveals logged (`pixels.revealed`: how
-- many, and, by the `pixels_where` command, where the last one is now).
local finding = ui.shared("pixels.finding", false)
local at = ui.shared("pixels.at", { "", 0, 0 } :: { any })
local revealed = ui.shared("pixels.revealed", "")
local last_revealed: Ref? = nil
-- `pixels.scroll`: the reply in a scroll, its find's block brought into view
-- as a transcript brings it (ds/transcript's `reveal`).
local scrolled = ui.shared("pixels.scroll", false)
local offset = ui.state("pixels_offset", 0)
local view = ui.ref()
local function bring(block: Ref)
	local el, v = block:peek() :: any, view:peek() :: any
	if not el or not v or el.y == nil or v.y == nil or el.height == nil or v.height == nil then
		return
	end
	local top, h = v.y :: number, v.height :: number
	local y, bh = el.y :: number, el.height :: number
	if y >= top + 8 and y + math.min(bh, 120) <= top + h - 8 then
		return
	end
	offset:set(math.max(0, offset:peek() + (y - top) - h / 3))
end
ui.command {
	id = "pixels_where",
	title = "Where the block last revealed is",
	run = function()
		local el = if last_revealed then last_revealed:peek() :: any else nil
		revealed:set(`{revealed:peek()}{if el then math.floor(el.y) else "-"};`)
	end,
}
local find: Prose.Find? = if finding:peek()
	then {
		key = "pixels",
		query = function(): string
			return tostring(at:get()[1])
		end,
		current = function(): Prose.FindAt?
			local c = at:get()
			if c[2] == 0 then
				return nil
			end
			return { key = "pixels", block = c[2] :: number, k = c[3] :: number }
		end,
		reveal = function(block: Ref)
			last_revealed = block
			revealed:set(`{revealed:peek()}r`)
			if scrolled:peek() then
				bring(block)
			end
		end,
	}
	else nil
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
if renderer:peek() == "native" then
	-- As a transcript's reply (ds/markdown's native path).
	reply = Markdown {
		text = words,
		native = true,
		key = if key:peek() ~= "" then key:peek() else nil,
		veil = veiled,
		veil_seed = seed:peek(),
		link_chip = link_chip,
		attached = attached,
		find = find,
		words = {
			at = function(): number?
				return if live:get() then 1 else nil
			end,
			states = { live_words },
		},
	}
elseif renderer:peek() ~= "luau" then
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
			find = Prose.find(find),
			block = Prose.block {
				text = words,
				key = if key:peek() ~= "" then key:peek() else nil,
				veil = veiled,
				veil_seed = seed:peek(),
				link_chip = link_chip,
				attached = attached,
			},
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
		find = find,
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
	if scrolled:peek()
		then ui.scroll {
			ref = view,
			offset = offset,
			w_full = true,
			h_full = true,
			ui.column { id = "reply", w_full = true, min_w = 0, reply },
		}
		else ui.column { id = "reply", w_full = true, min_w = 0, reply },
}

-- The accent Zeron is drawn in: the theme's own, or one of Zeron's presets
-- (crates/theme AccentPreset), each the theme's (`zeron.accents`, lighter
-- on dark, deeper on light, as AccentPreset::color has them). One setting
-- for the whole app, kept across launches, as the material is
-- (ds/material); a window takes it through `theme` (`look.theme()`).

local material = require("@self/material")
local locale = require("@self/locale")

local M = {}

export type Accent = { value: string, label: string, preset: boolean? }

-- An accent, its name read as a string in the locale shown (`a.label`,
-- through its message as it's asked for: read in a function, it follows).
-- Zeron's own is its name in every language.
local function accent(value: string, preset: boolean?): Accent
	return setmetatable({ value = value, preset = preset }, {
		__index = function(_, field: string): string?
			if field == "label" then
				return if value == "zeron" then "Zeron" else locale.s(`ds.look.accent.{value}`)
			end
			return nil
		end,
	}) :: any
end

-- In Zeron's order: the theme's own, then the presets.
M.ACCENTS = {
	accent("default"),
	accent("zeron", true),
	accent("orange", true),
	accent("amber", true),
	accent("green", true),
	accent("cyan", true),
	accent("blue", true),
	accent("pink", true),
} :: { Accent }

M.accent = ui.persist("accent", "default")

function M.find(value: string): Accent
	for _, a in M.ACCENTS do
		if a.value == value then
			return a
		end
	end
	return M.ACCENTS[1]
end

-- An accent's ink, in whichever scheme draws it. The theme's own is its
-- own token (`zeron.accents.default`, the theme's `accent` as written), so
-- it keeps its colour inside a window that wears another.
function M.ink(a: Accent): string
	return `$zeron.accents.{a.value}`
end

-- The tokens a window overrides to wear `value` (the chosen accent by
-- default): its `accent`, and what's derived from it follows. Nothing for
-- the theme's own.
function M.theme(value: string?): { [string]: any }?
	local a = M.find(value or M.accent:get())
	if not a.preset then
		return nil
	end
	return { accent = `$zeron.accents.{a.value}` }
end

-- The interface's text size: its body size in points (the theme's
-- `font.size.md`, 13, by default), every other text size and line height
-- scaled with it, the conversation's too (`zeron.prose.*`). Code has its
-- own (Settings → Appearance). Kept across launches, for the main window.
M.TEXT_SIZES = { 12, 13, 14, 15, 16 }
M.text_size = ui.persist("text_size", 13)

-- The tokens text is sized by, each with its line height beside it.
local SIZED = {
	"font.size.xs", "font.size.sm", "font.size.md", "font.size.lg", "font.size.xl", "font.size.xxl",
	"font.line.xs", "font.line.sm", "font.line.md", "font.line.lg", "font.line.xl", "font.line.xxl",
	"zeron.prose.size", "zeron.prose.line",
	"zeron.prose.h1.size", "zeron.prose.h1.line",
	"zeron.prose.h2.size", "zeron.prose.h2.line",
	"zeron.prose.h3.size", "zeron.prose.h3.line",
	"zeron.prose.small.size", "zeron.prose.small.line",
}

-- The text tokens a window overrides for `size` (the chosen one by
-- default): each scaled from the theme's own by size / 13, sizes to half
-- a point, line heights to whole points. Nothing at the theme's own size.
function M.text_theme(size: number?): { [string]: any }?
	local body = size or M.text_size:get()
	local base = (ui.theme.get("font.size.md") :: number?) or 13
	if body == base then
		return nil
	end
	local scale = body / base
	local out: { [string]: any } = {}
	for _, token in SIZED do
		local v = ui.theme.get(token) :: number?
		if type(v) == "number" then
			local line = string.find(token, "line", 1, true) ~= nil
			out[token] = if line then math.floor(v * scale + 0.5) else math.floor(v * scale * 2 + 0.5) / 2
		end
	end
	return out
end

-- The interface's font and code's (Settings → Appearance, as the shipping
-- app's Interface font and Code font): a family installed on this computer,
-- by its name, or "" for the theme's own (Geist, Geist Mono). Code's sets
-- code blocks, the editor, diffs, tool output and terminals. Kept across
-- launches, for the main window.
M.interface_font = ui.persist("interface_font", "")
M.code_font = ui.persist("code_font", "")

-- The font tokens a window overrides for the families chosen (each a
-- family's name, "" for the theme's own; the chosen ones by default).
-- Nothing for the theme's own. (Quoted, so a name is read as a family
-- whatever it looks like, "3270 Nerd Font" too, never a number or a colour.)
function M.font_theme(interface: string?, code: string?): { [string]: any }?
	local sans = interface or M.interface_font:get()
	local mono = code or M.code_font:get()
	if sans == "" and mono == "" then
		return nil
	end
	local out: { [string]: any } = {}
	if sans ~= "" then
		out["font.sans"] = `"{sans}"`
	end
	if mono ~= "" then
		out["font.mono"] = `"{mono}"`
	end
	return out
end

-- Code's family by its name, for what takes a family rather than a token
-- (a span of inline code): the one chosen, else the theme's own.
function M.code_family(): string
	local chosen = M.code_font:get()
	if chosen ~= "" then
		return chosen
	end
	local own = ui.theme.get("font.mono")
	return if type(own) == "string" and own ~= "" then own else "mono"
end

-- What the main window wears over its theme (ds/palettes, worn by
-- ds/tokens): the text size, the fonts chosen, and an accent chosen over
-- the theme's own.
function M.window_theme(): { [string]: any }?
	local out: { [string]: any }? = nil
	for _, part in { M.text_theme(), M.font_theme(), M.theme() } do
		if part then
			out = out or {}
			for k, v in part do
				(out :: any)[k] = v
			end
		end
	end
	-- And its glass's tint where the desktop blurs less than macOS's does
	-- (ds/material).
	local glass = material.glass_theme()
	if glass then
		out = out or {}
		for k, v in glass do
			(out :: any)[k] = v
		end
	end
	-- And its muted text where the window is solid (ds/material).
	local solid = material.solid_theme()
	if solid then
		out = out or {}
		for k, v in solid do
			(out :: any)[k] = v
		end
	end
	return out
end

return M

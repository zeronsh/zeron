-- The accent Zeron is drawn in: the theme's own, or one of Zeron's presets
-- (crates/theme AccentPreset), each the theme's (`zeron.accents`, lighter
-- on dark, deeper on light, as AccentPreset::color has them). One setting
-- for the whole app, kept across launches, as the material is
-- (ds/material); a window takes it through `theme` (`look.theme()`).

local palettes = require("@self/palettes")
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

-- The palette each scheme wears (ds/palettes; Zeron's own by default):
-- one for light, one for dark, as the shipping app keeps them.
M.palette = {
	light = ui.persist("palette_light", "zeron-light"),
	dark = ui.persist("palette_dark", "zeron-dark"),
}

-- The palette's tokens for the scheme showing.
function M.palette_theme(): { [string]: any }?
	local kind = ui.theme.kind()
	local light = kind == "light" or kind == "high-contrast-light"
	local p = palettes.find((if light then M.palette.light else M.palette.dark):get())
	return if p then palettes.tokens(p) else nil
end

-- What the main window wears over its template's theme: the text size, and
-- an accent chosen over the theme's own.
function M.window_theme(): { [string]: any }?
	-- (No palette over the theme: a template brings its own, so a palette
	-- chosen before the pickers went never applies.)
	local palette = nil
	local out: { [string]: any }? = nil
	for _, part in { palette, M.text_theme(), M.theme() } do
		if part then
			out = out or {}
			for k, v in part do
				(out :: any)[k] = v
			end
		end
	end
	return out
end

return M

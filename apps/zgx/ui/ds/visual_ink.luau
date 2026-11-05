-- An agent visual's ink: the colour words its script may use (crates/visual
-- style.rs: `text`, `muted`, `faint`, `accent`, `on-accent`, `success`,
-- `warning`, `danger`, `border`, `chart1` to `chart6`) as this design
-- system's theme tokens, so a visual follows the theme, light or dark, and
-- any template's. And its numbers as the script formats them.
--
--   ink.paint(color)       a token ("$accent"), or for a colour the script
--                          mixed or faded, its value in the theme now
--   ink.canvas(color)      the same for a canvas's paint (a plain string)
--   ink.hex(color)         `#rrggbbaa` in the theme now (a scene's, a
--                          shader's: drawn off the window)
--   ink.series(i)          the i-th chart colour (1-based, cycling)
--   ink.format(n, fmt)     `fmt` = { prefix, unit, decimals? }, as
--                          crates/visual `NumberFormat`: 1,234.5 ms
--
-- The chart colours: the accent first, then five fixed hues of the six
-- (`zeron.visual.series.*`), the one nearest the accent's hue left out, so
-- no two series read alike whatever the accent (the shipping app's rule).

local M = {}

local TOKENS: { [string]: string } = {
	text = "$text.primary",
	muted = "$text.secondary",
	faint = "$text.tertiary",
	accent = "$accent",
	["on-accent"] = "$accent.text",
	success = "$success",
	warning = "$warning",
	danger = "$danger",
	["border"] = "$border",
}

-- The fixed hues, by their hue in turns (OKLCH-ish, enough to tell which
-- sits nearest the accent).
local HUES = {
	{ name = "teal", hue = 0.486 },
	{ name = "amber", hue = 0.106 },
	{ name = "rose", hue = 0.964 },
	{ name = "violet", hue = 0.728 },
	{ name = "lime", hue = 0.236 },
	{ name = "sky", hue = 0.553 },
}

local function channels(hex: string): (number, number, number, number)
	local r = tonumber(string.sub(hex, 2, 3), 16) or 0
	local g = tonumber(string.sub(hex, 4, 5), 16) or 0
	local b = tonumber(string.sub(hex, 6, 7), 16) or 0
	local a = if #hex >= 9 then tonumber(string.sub(hex, 8, 9), 16) or 255 else 255
	return r / 255, g / 255, b / 255, a / 255
end

-- A colour's hue in turns and its saturation.
local function hue_of(hex: string): (number, number)
	local r, g, b = channels(hex)
	local hi, lo = math.max(r, g, b), math.min(r, g, b)
	local d = hi - lo
	if d < 1e-6 then
		return 0, 0
	end
	local h
	if hi == r then
		h = ((g - b) / d) % 6
	elseif hi == g then
		h = (b - r) / d + 2
	else
		h = (r - g) / d + 4
	end
	local l = (hi + lo) / 2
	local s = d / (1 - math.abs(2 * l - 1))
	return h / 6, s
end

-- The series tokens for the theme now (read tracked: inside a computed or a
-- draw, a theme change picks them again).
local function series_tokens(): { string }
	ui.theme.current:get()
	local accent = ui.theme.get("accent")
	local skip = #HUES
	if type(accent) == "string" then
		local h, s = hue_of(accent)
		if s >= 0.15 then
			local best = math.huge
			for i, hue in HUES do
				local d = math.abs(hue.hue - h) % 1
				d = math.min(d, 1 - d)
				if d < best then
					best, skip = d, i
				end
			end
		end
	end
	local out = { "$accent" }
	for i, hue in HUES do
		if i ~= skip and #out < 6 then
			table.insert(out, `$zeron.visual.series.{hue.name}`)
		end
	end
	return out
end

function M.series(i: number): string
	local list = series_tokens()
	return list[((i - 1) % #list) + 1]
end

-- The theme token a colour word names (nil for a physical colour, which
-- only scenes take).
local function token_of(name: string): string?
	local plain = TOKENS[name]
	if plain then
		return plain
	end
	local n = tonumber(string.match(name, "^chart(%d)$"))
	if n then
		return M.series(n)
	end
	return nil
end

-- A token's value now, as `#rrggbbaa`.
local function resolve(name: string): string
	if string.sub(name, 1, 1) == "#" then
		return if #name == 7 then name .. "ff" else name
	end
	local token = token_of(name) or "$text.primary"
	local v = ui.theme.get(string.sub(token, 2))
	return if type(v) == "string" then v else "#808080ff"
end

local function to_hex(r: number, g: number, b: number, a: number): string
	local function byte(x: number): number
		return math.clamp(math.floor(x * 255 + 0.5), 0, 255)
	end
	return string.format("#%02x%02x%02x%02x", byte(r), byte(g), byte(b), byte(a))
end

-- Mixed and faded colours made, by what they're made of (at most 4096).
local MIXED: { [string]: string } = {}
local MIXED_COUNT = 0

-- A colour (a word, or `{ token, mix?, t?, alpha }`) as `#rrggbbaa` now.
function M.hex(color: any): string
	if type(color) == "string" then
		return resolve(color)
	end
	if type(color) ~= "table" then
		return resolve("text")
	end
	ui.theme.current:get()
	local base = resolve(color.token or "text")
	local other = if color.mix then resolve(color.mix) else nil
	-- (Kept by the colours it's made of, as they are now: a canvas drawing
	-- every frame, or hundreds of marks, asks again for the same ones.)
	local key = `{base}{other or ""}{color.t or ""}/{color.alpha or 1}`
	local kept = MIXED[key]
	if kept then
		return kept
	end
	local r, g, b, a = channels(base)
	if other then
		local r2, g2, b2, a2 = channels(other)
		local t = color.t or 0.5
		r, g, b, a = r + (r2 - r) * t, g + (g2 - g) * t, b + (b2 - b) * t, a + (a2 - a) * t
	end
	local out = to_hex(r, g, b, a * (color.alpha or 1))
	if MIXED_COUNT >= 4096 then
		MIXED, MIXED_COUNT = {}, 0
	end
	MIXED[key] = out
	MIXED_COUNT += 1
	return out
end

-- How a colour paints a node or a canvas: its token while it is one, so it
-- follows the theme without the visual drawing again; else its value now.
function M.paint(color: any, fallback: string?): any
	if color == nil then
		return fallback or "$text.primary"
	end
	if type(color) == "string" then
		local token = token_of(color)
		if token then
			return token
		end
		if string.sub(color, 1, 1) == "#" then
			return ui.raw(color, "a visual's physical colour (only scenes have them)")
		end
		return fallback or "$text.primary"
	end
	return ui.raw(M.hex(color), "a visual's colour the script mixed or faded")
end

-- How a colour paints on a canvas: its token while it is one (the canvas
-- follows the theme), else its value now as plain `#rrggbbaa` (a canvas
-- takes a string, not a node style's `ui.raw`).
function M.canvas(color: any, fallback: string?): string
	if color == nil then
		return fallback or "$text.primary"
	end
	if type(color) == "string" then
		local token = token_of(color)
		if token then
			return token
		end
		if string.sub(color, 1, 1) == "#" then
			return resolve(color)
		end
		return fallback or "$text.primary"
	end
	return M.hex(color)
end

-- Every colour word's value now, for a scene or shader drawn off the window.
function M.palette(): { [string]: string }
	local out: { [string]: string } = {}
	for name in TOKENS do
		out[name] = resolve(name)
	end
	for i = 1, 6 do
		out[`chart{i}`] = resolve(`chart{i}`)
	end
	return out
end

-- A number as the script formats it (crates/visual `NumberFormat::format`):
-- grouped thousands, its decimals (or 0 for whole numbers and from 100 up,
-- 1 from 10, else 2), the prefix before and the unit after (a space before
-- it, unless it is a percent).
function M.format(value: number, fmt: any?): string
	local f = fmt or {}
	local decimals = f.decimals
	if decimals == nil then
		local m = math.abs(value)
		if m == 0 or m >= 100 or value % 1 == 0 then
			decimals = 0
		elseif m >= 10 then
			decimals = 1
		else
			decimals = 2
		end
	end
	local text = string.format(`%.{decimals}f`, value)
	local sign = ""
	if string.sub(text, 1, 1) == "-" then
		sign, text = "-", string.sub(text, 2)
	end
	local int, frac = string.match(text, "^(%d+)(.*)$")
	int = int or text
	local grouped = string.reverse((string.gsub(string.reverse(int), "(%d%d%d)", "%1,")))
	if string.sub(grouped, 1, 1) == "," then
		grouped = string.sub(grouped, 2)
	end
	local unit = f.unit or ""
	local space = if unit == "" or string.sub(unit, 1, 1) == "%" then "" else " "
	return `{sign}{f.prefix or ""}{grouped}{frac or ""}{space}{unit}`
end

-- The transcript's type scale for a visual's sizes (`zeron.visual.text.*`):
-- size and line, in points.
function M.size(name: string?): (string, string)
	local n = if name == "2xl" then "xxl" else (name or "body")
	return `$zeron.visual.text.{n}.size`, `$zeron.visual.text.{n}.line`
end

M.WEIGHTS = {
	regular = "$font.weight.regular",
	medium = "$font.weight.medium",
	semibold = "$font.weight.semibold",
	bold = "$font.weight.bold",
}

return M

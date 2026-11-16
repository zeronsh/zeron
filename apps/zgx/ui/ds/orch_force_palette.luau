-- The force Graph's machine colours (ds/orch_force): a machine's chats and
-- orchestrators filled with its colour, soft and pastel, never one a status
-- wears. The statuses' hues are read from the theme (`$zeron.status.*`,
-- their dots, the tones and the pull request badges the sidebar shows), and
-- every machine hue keeps MIN_HUE away from each of them (OKLCH hue), so a
-- fill never reads as working, done, failed, asking or waiting; none is
-- grey. The hues left are taken farthest-first (each the one most apart
-- from those taken), so the first machines are the most distinct, the same
-- every time for the same theme.
--
--   pal.machines(dark?)        { hex } for this theme (cached by theme)
--   pal.make(status_hexes, dark)  the same from given statuses (pure)
--   pal.mark_ink(fill)         black or white, whichever contrasts more
--   pal.contrast(a, b)         WCAG contrast ratio of two hexes
--   pal.hue_of(hex)            OKLCH lightness, chroma, hue

local M = {}

-- The theme's colours a machine's must stay clear of.
M.STATUS_TOKENS = {
	"zeron.status.working",
	"zeron.status.connecting",
	"zeron.status.waiting",
	"zeron.status.input",
	"zeron.status.approval",
	"zeron.status.plan",
	"zeron.status.done",
	"zeron.status.failed",
	"zeron.status.limited",
	"zeron.status.dot.working",
	"zeron.status.dot.input",
	"zeron.status.dot.approval",
	"zeron.status.dot.done",
	"zeron.status.dot.failed",
	"zeron.tone.info.ink",
	"zeron.tone.accent.ink",
	"zeron.tone.success.ink",
	"zeron.tone.warning.ink",
	"zeron.tone.danger.ink",
	"zeron.pr.open",
	"zeron.pr.merged",
	"zeron.pr.closed",
}
-- A status colour with less chroma than this is a grey (it has no hue to
-- keep clear of); a machine's always has more than GREY_C.
M.GREY_C = 0.04
-- How far (degrees of OKLCH hue) a machine's colour keeps from a status's,
-- and from another machine's.
M.MIN_HUE = 28
M.MIN_APART = 16
-- How many machine colours there are (more machines share them, in turn).
M.COUNT = 10
-- The tones: pastel, each theme its own (lighter on dark; on light, deep
-- enough to stand off the page, light enough under a black mark).
M.TONE = {
	dark = { l = 0.82, c = 0.075 },
	light = { l = 0.8, c = 0.095 },
}
M.DARK_INK = "#000000"
M.LIGHT_INK = "#ffffff"

local function rgb_of(hex: string): (number, number, number)
	return (tonumber(string.sub(hex, 2, 3), 16) or 0) / 255, (tonumber(string.sub(hex, 4, 5), 16) or 0) / 255, (tonumber(string.sub(hex, 6, 7), 16) or 0) / 255
end
local function lin(v: number): number
	return if v <= 0.04045 then v / 12.92 else ((v + 0.055) / 1.055) ^ 2.4
end
local function gam(v: number): number
	return if v <= 0.0031308 then 12.92 * v else 1.055 * v ^ (1 / 2.4) - 0.055
end

-- OKLCH of a hex: lightness, chroma, hue (degrees).
function M.hue_of(hex: string): (number, number, number)
	local r0: number, g0: number, b0: number = rgb_of(hex)
	local r: number, g: number, b: number = lin(r0), lin(g0), lin(b0)
	local l: number = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b) ^ (1 / 3)
	local m: number = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b) ^ (1 / 3)
	local s: number = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b) ^ (1 / 3)
	local L = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s
	local A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s
	local B = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s
	return L, math.sqrt(A * A + B * B), math.deg(math.atan2(B, A)) % 360
end

-- sRGB (0–1, maybe out of gamut) of an OKLCH colour.
local function srgb(L: number, C: number, h: number): (number, number, number)
	local A, B = C * math.cos(math.rad(h)), C * math.sin(math.rad(h))
	local l = (L + 0.3963377774 * A + 0.2158037573 * B) ^ 3
	local m = (L - 0.1055613458 * A - 0.0638541728 * B) ^ 3
	local s = (L - 0.0894841775 * A - 1.291485548 * B) ^ 3
	return gam(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s), gam(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s), gam(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s)
end

-- A hex of an OKLCH colour, its chroma eased down until it's in sRGB.
function M.oklch(L: number, C: number, h: number): string
	local c = C
	local r, g, b = srgb(L, c, h)
	while c > 0 and (r < 0 or r > 1 or g < 0 or g > 1 or b < 0 or b > 1) do
		c -= 0.002
		r, g, b = srgb(L, c, h)
	end
	local function byte(v: number): number
		return math.clamp(math.floor(v * 255 + 0.5), 0, 255)
	end
	return string.format("#%02x%02x%02x", byte(r), byte(g), byte(b))
end

-- WCAG 2 relative luminance and contrast ratio.
local function luminance(hex: string): number
	local r: number, g: number, b: number = rgb_of(hex)
	return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
end
function M.contrast(a: string, b: string): number
	local la, lb = luminance(a), luminance(b)
	return (math.max(la, lb) + 0.05) / (math.min(la, lb) + 0.05)
end

-- A mark's ink on `fill`: black or white, whichever has the higher contrast
-- ratio with it.
function M.mark_ink(fill: string): string
	return if M.contrast(M.DARK_INK, fill) >= M.contrast(M.LIGHT_INK, fill) then M.DARK_INK else M.LIGHT_INK
end

local function apart(a: number, b: number): number
	local d = math.abs(a - b) % 360
	return math.min(d, 360 - d)
end

-- The machine colours, from the statuses' colours (hexes): the hues at
-- least MIN_HUE from every chromatic one, taken farthest-first.
function M.make(statuses: { string }, dark: boolean): { string }
	local avoid: { number } = {}
	for _, hex in statuses do
		if type(hex) == "string" and string.sub(hex, 1, 1) == "#" and #hex >= 7 then
			local _, c, h = M.hue_of(hex)
			if c >= M.GREY_C then
				table.insert(avoid, h)
			end
		end
	end
	local tone = if dark then M.TONE.dark else M.TONE.light
	-- (A theme whose statuses fill the wheel: the margin eased until there
	-- are hues enough.)
	local margin = M.MIN_HUE
	local hues: { number } = {}
	while true do
		table.clear(hues)
		local free: { number } = {}
		for h = 0, 359 do
			local ok = true
			for _, a in avoid do
				if apart(h, a) < margin then
					ok = false
					break
				end
			end
			if ok then
				table.insert(free, h)
			end
		end
		-- Farthest-first: the first the farthest from any status, each next
		-- the farthest from those taken (a tie to the lower hue).
		while #hues < M.COUNT and #free > 0 do
			local best, best_d = -1, -1
			for _, h in free do
				local d = math.huge
				if #hues == 0 then
					for _, a in avoid do
						d = math.min(d, apart(h, a))
					end
				else
					for _, t in hues do
						d = math.min(d, apart(h, t))
					end
				end
				if d > best_d then
					best, best_d = h, d
				end
			end
			if #hues > 0 and best_d < M.MIN_APART then
				break
			end
			table.insert(hues, best)
		end
		if #hues >= 6 or margin <= 12 then
			break
		end
		margin -= 4
	end
	local out: { string } = {}
	for _, h in hues do
		table.insert(out, M.oklch(tone.l, tone.c, h))
	end
	return out
end

-- This theme's machine colours (worked out once a theme).
local made: { [string]: { string } } = {}
function M.status_inks(): { string }
	local out: { string } = {}
	for _, t in M.STATUS_TOKENS do
		local v = ui.theme.get(t)
		if type(v) == "string" then
			table.insert(out, v)
		end
	end
	return out
end
function M.machines(dark: boolean): { string }
	local key = `{ui.theme.current:peek()}/{dark}`
	local kept = made[key]
	if not kept then
		kept = M.make(M.status_inks(), dark)
		made[key] = kept
	end
	return kept
end

return M

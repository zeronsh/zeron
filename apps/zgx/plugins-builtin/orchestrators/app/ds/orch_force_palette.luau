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

-- The statuses a node on the Map wears (its ring, the sidebar's glyph): a
-- machine's colour must never read as one of them.
M.STATUS_TOKENS = {
	"zeron.status.working",
	"zeron.status.connecting",
	"zeron.status.waiting",
	"zeron.status.input",
	"zeron.status.approval",
	"zeron.status.plan",
	"zeron.status.done",
	"zeron.status.failed",
}
-- A status colour with less chroma than this is a grey (it has no hue to
-- keep clear of); a machine's always has more than GREY_C.
M.GREY_C = 0.04
-- How far (degrees of OKLCH hue) a machine's colour keeps from a status's
-- (coral, not error red; yellow-green, not done green), and from another
-- machine's.
M.MIN_HUE = 18
M.MIN_APART = 24
-- How many machine colours there are, spread evenly round the wheel (more
-- machines share them, in turn).
M.COUNT = 8
-- The tones: pastel, each theme its own (lighter on dark; on light, deep
-- enough to stand off the page, light enough under a black mark).
-- Each neighbour on the wheel this much lighter or darker (OKLCH L).
M.LIFT = 0.04
M.TONE = {
	dark = { l = 0.83, c = 0.1 },
	light = { l = 0.79, c = 0.105 },
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
	-- (In a steady order, whatever order they came in.)
	table.sort(avoid)
	local tone = if dark then M.TONE.dark else M.TONE.light
	-- A hue off every status: itself if clear, else the nearer edge of the
	-- status's margin (pushed on past any other status there).
	local function clear(h: number): number
		for _ = 1, 8 do
			local hit: number? = nil
			for _, a in avoid do
				if apart(h, a) < M.MIN_HUE then
					hit = a
					break
				end
			end
			if not hit then
				return h % 360
			end
			local d = ((h - hit + 540) % 360) - 180
			h = hit + (if d >= 0 then M.MIN_HUE else -M.MIN_HUE)
		end
		return h % 360
	end
	-- COUNT hues evenly round the wheel, turned (by a degree at a time) to
	-- where, once each is cleared of the statuses, they're most apart.
	local step = 360 / M.COUNT
	local best: { number } = {}
	local best_gap = -1
	for turn = 0, math.floor(step) - 1 do
		local set: { number } = {}
		for k = 0, M.COUNT - 1 do
			table.insert(set, clear(turn + k * step))
		end
		local gap = math.huge
		for i = 1, #set do
			for j = i + 1, #set do
				gap = math.min(gap, apart(set[i], set[j]))
			end
		end
		if gap > best_gap then
			best, best_gap = set, gap
		end
	end
	-- (Neighbours round the wheel a step apart in lightness too: two soft
	-- colours a hue apart still read as two.)
	local lift: { [number]: number } = {}
	local by_hue = table.clone(best)
	table.sort(by_hue)
	for k, h in by_hue do
		lift[h] = if k % 2 == 0 then M.LIFT else -M.LIFT
	end
	-- Handed out farthest-first: the first machines the most apart.
	local hues: { number } = {}
	local left = table.clone(best)
	-- (The first: the one farthest from any status.)
	local function far_from(list: { number }, h: number): number
		local d = math.huge
		for _, t in list do
			d = math.min(d, apart(h, t))
		end
		return d
	end
	while #left > 0 do
		local pick, pick_d = 1, -1
		for k, h in left do
			local d = if #hues == 0 then far_from(avoid, h) else far_from(hues, h)
			if d > pick_d then
				pick, pick_d = k, d
			end
		end
		table.insert(hues, table.remove(left, pick) :: number)
	end
	local out: { string } = {}
	for _, h in hues do
		table.insert(out, M.oklch(tone.l + (lift[h] or 0), tone.c, h))
	end
	return out
end

-- The difference two colours show (ΔE in OKLab, ×100: about 2 is a just
-- seen difference; past 10, clearly two colours).
function M.delta(a: string, b: string): number
	local la, ca, ha = M.hue_of(a)
	local lb, cb, hb = M.hue_of(b)
	local aa, ab = ca * math.cos(math.rad(ha)), ca * math.sin(math.rad(ha))
	local ba, bb = cb * math.cos(math.rad(hb)), cb * math.sin(math.rad(hb))
	return math.sqrt((la - lb) ^ 2 + (aa - ba) ^ 2 + (ab - bb) ^ 2) * 100
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

-- The machines' colours as theme tokens, `$zeron.force.machine.m1`… (this
-- theme's own, declared again as the theme changes): what a style wears
-- (the legend's dots); the canvas draws the same colours from `machines`.
function M.token(k: number): string
	return `$zeron.force.machine.m{k}`
end
local declared: string? = nil
function M.declare(dark: boolean)
	local key = `{ui.theme.current:peek()}/{dark}`
	if declared == key then
		return
	end
	declared = key
	local tokens: { [string]: string } = {}
	for k, c in M.machines(dark) do
		tokens[`m{k}`] = c
	end
	ui.theme.tokens { zeron = { force = { machine = tokens } } }
end

return M

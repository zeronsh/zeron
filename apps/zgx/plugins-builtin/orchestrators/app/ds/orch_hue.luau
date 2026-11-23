-- An orchestrator's hue: the one colour orchestration adds, the thread that
-- ties the map, the board, a dump's receipt and the sidebar together. Each
-- orchestrator has a hue (0–359, its row's `hue`); this turns it into paint
-- at one lightness and chroma for each scheme (OKLCH, so every hue reads
-- as bright as the next), the scheme read from the theme's own ink, so it
-- follows a theme change like a token does.
--
--   orch_hue.ink(hue)            `#rrggbbaa`: lines, dots, a name's mark (a
--                                canvas's paint)
--   orch_hue.canvas(hue, alpha)  the same at `alpha` (a canvas's)
--   orch_hue.paint(hue, alpha?)  for a node's `bg`/`color`/`border_color`
--   orch_hue.wash(hue)           a tint to lay under something of its own
--                                (a receipt's excerpt, a selected row)
--   orch_hue.line(hue)           its edges' paint (the map's tracks)
--   orch_hue.neutral             what has no hue (the inbox, a note)
--   orch_hue.Dot { hue, size?, pulse? }
--                                its dot: `hue` a number or a function of
--                                one (nil: neutral); `pulse` a function, true
--                                while something of it works (it breathes,
--                                still with motion reduced)

local motion = require("@ds/motion")
-- (ds/status, asked for as it's needed: a cycle at load.)
local function status(): any
	return require("@ds/status")
end

local M = {}

-- Lightness and chroma, per scheme: on dark glass a lighter, softer ink;
-- on light, deeper. The wash and the line are the ink, thinned.
local DARK = { l = 0.76, c = 0.125, wash = 0.17, line = 0.62 }
local LIGHT = { l = 0.56, c = 0.135, wash = 0.13, line = 0.7 }

M.neutral = -1

local function srgb(x: number): number
	local v = if x <= 0.0031308 then 12.92 * x else 1.055 * math.pow(x, 1 / 2.4) - 0.055
	return v
end

-- OKLCH to sRGB (0–1 each), the chroma brought in until it fits.
local function rgb(l: number, c: number, h: number): (number, number, number)
	local rad = math.rad(h)
	for _ = 1, 12 do
		local a, b = c * math.cos(rad), c * math.sin(rad)
		local l_ = l + 0.3963377774 * a + 0.2158037573 * b
		local m_ = l - 0.1055613458 * a - 0.0638541728 * b
		local s_ = l - 0.0894841775 * a - 1.2914855480 * b
		local L, Mv, S = l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_
		local r = 4.0767416621 * L - 3.3077115913 * Mv + 0.2309699292 * S
		local g = -1.2684380046 * L + 2.6097574011 * Mv - 0.3413193965 * S
		local bl = -0.0041960863 * L - 0.7034186147 * Mv + 1.7076147010 * S
		if r >= -0.001 and r <= 1.001 and g >= -0.001 and g <= 1.001 and bl >= -0.001 and bl <= 1.001 then
			return srgb(math.clamp(r, 0, 1)), srgb(math.clamp(g, 0, 1)), srgb(math.clamp(bl, 0, 1))
		end
		c *= 0.88
	end
	return srgb(l), srgb(l), srgb(l)
end

local function byte(x: number): number
	return math.clamp(math.floor(x * 255 + 0.5), 0, 255)
end

-- Whether the theme now is a dark one: its primary ink is light. (Tracked:
-- read in a binding, a theme change paints again.)
local function dark(): boolean
	ui.theme.current:get()
	local ink = ui.theme.get("text.primary")
	if type(ink) ~= "string" or string.sub(ink, 1, 1) ~= "#" then
		return true
	end
	local r = (tonumber(string.sub(ink, 2, 3), 16) or 0) / 255
	local g = (tonumber(string.sub(ink, 4, 5), 16) or 0) / 255
	local b = (tonumber(string.sub(ink, 6, 7), 16) or 0) / 255
	return 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.5
end
M.dark = dark

-- What's made, by scheme, hue and alpha (a few dozen at most).
local MADE: { [string]: string } = {}

local function hex(hue: number?, alpha: number): string
	local is_dark = dark()
	local key = `{if is_dark then "d" else "l"}{hue or -1}/{alpha}`
	local kept = MADE[key]
	if kept then
		return kept
	end
	local out
	if hue == nil or hue < 0 then
		-- No hue: the theme's own secondary ink.
		local ink = ui.theme.get("text.secondary")
		local base = if type(ink) == "string" and #ink >= 7 then string.sub(ink, 1, 7) else "#808080"
		local a = if type(ink) == "string" and #ink >= 9 then (tonumber(string.sub(ink, 8, 9), 16) or 255) / 255 else 1
		out = base .. string.format("%02x", byte(a * alpha))
	else
		local s = if is_dark then DARK else LIGHT
		local r, g, b = rgb(s.l, s.c, hue % 360)
		out = string.format("#%02x%02x%02x%02x", byte(r), byte(g), byte(b), byte(alpha))
	end
	MADE[key] = out
	return out
end

function M.ink(hue: number?): string
	return hex(hue, 1)
end

function M.canvas(hue: number?, alpha: number?): string
	return hex(hue, alpha or 1)
end

function M.paint(hue: number?, alpha: number?): any
	return ui.raw(hex(hue, alpha or 1), "an orchestrator's hue (ds/orch_hue)")
end

function M.wash_alpha(): number
	return if dark() then DARK.wash else LIGHT.wash
end

function M.wash(hue: number?): any
	return M.paint(hue, M.wash_alpha())
end

function M.line(hue: number?): string
	return hex(hue, if dark() then DARK.line else LIGHT.line)
end

local function read(v: any): number?
	if type(v) == "function" then
		return (v :: any)()
	end
	return v
end

-- Its dot. Breathing (dimming to half and back) while `pulse` says so,
-- only while it can be seen, and never with motion reduced.
function M.Dot(props: { hue: any, size: number?, pulse: (() -> boolean)?, label: string? }): Node
	local size = props.size or 8
	local el_ref: Element? = nil
	local playing = false
	local seen = false
	local step_key = {}
	ui.on_cleanup(function()
		status().unstep(step_key)
	end)
	local function want(): boolean
		return props.pulse ~= nil and (props.pulse :: () -> boolean)()
	end
	local function sync()
		local el = el_ref
		if not el then
			return
		end
		local should = seen and want() and not motion.reduced_now()
		if status().stepped() then
			-- (Off macOS, stepped on the status clock: ds/status `step`.)
			if should and not playing then
				playing = true
				status().step(step_key, el, function(e: Element, now: number)
					anime.utils.set(e, { opacity = status().sine_at(now, 1, 0.45, 900) })
				end)
			elseif not should and playing then
				playing = false
				status().unstep(step_key)
				anime.utils.set(el, { opacity = 1 })
			end
			return
		end
		if should and not playing then
			playing = true
			local anim = anime.animate(el, {
				opacity = { 1, 0.45 },
				duration = 900,
				ease = "inOutSine",
				loop = true,
				alternate = true,
				frameRate = 30,
			})
			anim.currentTime = ui.now() % 1800
		elseif not should and playing then
			playing = false
			anime.utils.remove(el)
			anime.utils.set(el, { opacity = 1 })
		end
	end
	if props.pulse then
		ui.effect(function()
			want()
			ui.after(0, sync)
		end)
	end
	return ui.div {
		class = "orch-hue-dot",
		w = size,
		h = size,
		shrink = 0,
		rounded = "$radius.full",
		label = props.label,
		bg = function(): any
			return M.paint(read(props.hue))
		end,
		on_visible = if props.pulse
			then function(visible: boolean, el: Element)
				el_ref = el
				seen = visible
				sync()
			end
			else nil,
	}
end

return M

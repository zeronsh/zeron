-- How the window's columns move: the sidebar folding away and back, a
-- session's panel opening and closing, and what rides with them (the
-- page's title in the strip, the title bar's island). Zeron's own
-- (crates/ui `WidthTween::panel`, motion `PANEL_RESIZE`): the dialog
-- entrance's curve, CSS `ease`, easing into the move and landing softly;
-- 180 ms for the sidebar's width, longer moves taking longer by the square
-- root of how far they go, to at most 240 ms, so a wide panel travels at
-- the sidebar's pace instead of covering twice the distance in the same
-- time. Each column keeps its content at full width while it moves (its
-- edge clips it), so nothing in it lays out again on the way.
--
--   local motion = require("@ds/motion")
--   anime.animate(width, motion.column(from, to))     { value, duration, ease }
--   ui.tween(x, motion.sidebar)                        the sidebar's own move

local prefers = require("@self/prefers")

local M = {}

-- Reduce motion: the one place every animation asks. On with the system's
-- Reduce motion (macOS) or Zeron's own setting (`M.reduce`, Settings), and
-- then nothing travels: a move lands at once, a fade is a short crossfade,
-- and what a state change shows is right on the next frame. Tracked: read in
-- a binding or computed, it follows a change of either.
M.reduce = ui.persist("motion_reduce", false)
function M.reduced(): boolean
	return M.reduce:get() or prefers.reduced_motion:get()
end
-- The same, untracked (where a move starts: a handler, an effect).
function M.reduced_now(): boolean
	return M.reduce:peek() or prefers.reduced_motion:peek()
end
-- The longest a fade runs with motion reduced: a crossfade, not a move.
M.FADE = 120

-- A movement's duration as motion allows: `ms`, or nothing (reduced).
function M.ms(ms: number): number
	return if M.reduced_now() then 0 else ms
end

-- A value that moves to where `target` goes (`ui.tween`), or, with motion
-- reduced, `target` itself, so it lands at once: for a prop. Decided as
-- the part is built (a tween's options are fixed when it's made), so the
-- setting changed reaches what's built after it.
function M.tween(target: any, opts: TweenOptions): any
	return if M.reduced_now() then target else ui.tween(target, opts)
end

-- The same for `ui.spring`.
function M.spring(target: any, opts: SpringOptions): any
	return if M.reduced_now() then target else ui.spring(target, opts)
end

-- What of an animation is movement (it travels or grows), as opposed to a
-- fade or a colour.
local MOVES = {
	x = true, y = true, translateX = true, translateY = true, translate_x = true, translate_y = true,
	scale = true, scaleX = true, scaleY = true, rotate = true, w = true, h = true, width = true,
	height = true, value = true, left = true, top = true, right = true, bottom = true,
}

-- `anime.animate`'s params as motion allows: with motion reduced, a move
-- lands at once (no duration) and a fade alone is a short crossfade.
function M.params<P>(params: P): P
	if not M.reduced_now() then
		return params
	end
	local out = table.clone(params :: any)
	local moves = false
	for k in params :: any do
		if MOVES[k] then
			moves = true
		end
	end
	out.duration = if moves then 0 else math.min((params :: any).duration or M.FADE, M.FADE)
	out.delay = 0
	return out :: P
end

-- A presence's or an each's `enter`/`exit` as motion allows: with motion
-- reduced, its movement dropped and what's left (a fade) short. Read as
-- the part is built.
function M.transition(spec: { [string]: any }?): { [string]: any }?
	if not spec or not M.reduced_now() then
		return spec
	end
	local out: { [string]: any } = {}
	for k, v in spec do
		if not MOVES[k] then
			out[k] = v
		end
	end
	out.duration = math.min(spec.duration or M.FADE, M.FADE)
	out.delay = nil
	return out
end

-- An each's `animate` (rows sliding to new places) as motion allows:
-- nothing with motion reduced, so rows take their places at once.
function M.layout(spec: { [string]: any }?): { [string]: any }?
	return if M.reduced_now() then nil else spec
end

-- CSS `ease`: cubic-bezier(0.25, 0.1, 0.25, 1).
M.EASE = "cubicBezier(.25,.1,.25,1)"
-- The base, and the longest, in milliseconds; the sidebar's default width,
-- the distance the base covers.
M.BASE, M.LONGEST, M.SIDEBAR = 180, 240, 256

-- How long a column takes to move `distance` points.
function M.duration(distance: number): number
	local scale = math.clamp(math.sqrt(math.abs(distance) / M.SIDEBAR), 1, M.LONGEST / M.BASE)
	return M.BASE * scale
end

-- The panel's width going to `to`, for `anime.animate`: most of the way at once,
-- then settling, so it answers as soon as it's asked. (At once, with motion reduced.)
M.PANEL_MS = 280
M.PANEL_EASE = "cubicBezier(.22, 1, .36, 1)"
function M.panel(to: number): TableParams
	return { value = to, duration = M.ms(M.PANEL_MS), ease = M.PANEL_EASE }
end

-- A column's width from `from` to `to`, for `anime.animate` (its `value`
-- the width it goes to): at once, with motion reduced.
function M.column(from: number, to: number): TableParams
	return { value = to, duration = M.ms(M.duration(to - from)), ease = M.EASE }
end

-- What moves with the sidebar (its own width is the sidebar's default or
-- near it): for `ui.tween`.
M.sidebar = { duration = M.BASE, ease = M.EASE } :: TweenOptions

-- Snappy: what answers a press at once (the lightbox's picture growing out
-- of its thumbnail and going back, its zoom). It moves from the press's own
-- frame, fast at first and easing into place with no give: a strong
-- ease-out, `open` and `close` long, a zoom step `zoom` long. One place to
-- tune it.
M.SNAPPY = { open = 250, close = 200, zoom = 200, ease = "out(4)" }

-- A hover's fade (Zeron's `HOVER_FADE`: 150 ms on Tailwind's ease,
-- cubic-bezier(0.4, 0, 0.2, 1)): what the pointer lights fades in at once
-- and out as it leaves, from wherever it stood. `hover()` is it for
-- `ui.tween`, at once with motion reduced.
M.HOVER_MS, M.HOVER_EASE = 150, "cubicBezier(.4,0,.2,1)"
function M.hover(): TweenOptions
	return { duration = M.ms(M.HOVER_MS), ease = M.HOVER_EASE }
end

return M

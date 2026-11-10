-- How the window's columns move: the sidebar folding away and back, a
-- session's panel opening and closing, and what rides with them (the
-- page's title in the strip, the title bar's island). Zeron's own
-- (crates/ui `WidthTween::panel`, motion `PANEL_RESIZE`): the dialog
-- entrance's curve, CSS `ease`, easing into the move and landing softly;
-- 180 ms for the sidebar's width, longer moves taking longer by the square
-- root of how far they go, to at most 240 ms, so a wide panel travels at
-- the sidebar's pace instead of covering twice the distance in the same
-- time. Each column keeps its content at full width while it moves (its
-- edge clips it), so nothing in it lays out again on the way; what comes or
-- goes with it stands where it is and only fades a little (`content`).
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

-- What a column holds as its edge sweeps over it (the panel's tabs, the
-- page under an expanded panel, a split's new pane): laid out once, at the
-- width it's going to (or, going, the width it rested at), it never
-- squeezes and never moves: the edge alone uncovers it and covers it
-- again. It only fades a little as it comes or goes, from `FADE_FLOOR` to
-- full in `FADE_MS` (Zeron's quick fade, `FADE_QUICK`, CSS `ease`; the
-- floor is its menus' entrance's, 0.3 + 0.7t), so it softens under the
-- edge instead of being cut by it alone.
M.FADE_FLOOR = 0.3
M.FADE_MS = 150
-- `anime.animate`'s params for content coming in (`on`) or going: to full,
-- or to the floor, from where it shows. `from_floor`: it comes from the
-- floor (it stood fully hidden under what covered it, so the step's first
-- frame, still covered, shows it at no more than the floor). `at_once`
-- (the route moving, nobody's toggle) or with motion reduced, there on the
-- next frame.
function M.content(on: boolean, at_once: boolean?, from_floor: boolean?): ElementParams
	local opacity = if on then 1 else M.FADE_FLOOR
	if at_once or M.reduced_now() then
		return { opacity = opacity, duration = 0 } :: any
	end
	local fade: { [string]: any } = { to = opacity, duration = M.FADE_MS, ease = M.EASE }
	if from_floor then
		fade.from = M.FADE_FLOOR
	end
	local params: { [string]: any } = { opacity = fade }
	if on then
		-- (Coming in, it stands `SWEEP` points off for the edge's sweep, back
		-- at rest as the sweep lands: no eye sees a hundredth of a point,
		-- but what reads where it shows does, each frame. A list that builds
		-- only the rows that show (`virtualize = "ancestor"`) follows the
		-- scroll views around it, not the column's edge clipping them from
		-- outside: still, it would keep the rows it had as the sweep began,
		-- none.)
		params.x = { from = M.SWEEP, to = 0, duration = M.PANEL_MS, ease = "linear" }
	end
	return params :: any
end
M.SWEEP = 0.01

-- A seam (a column's edge, a split's divider) dragged past its limit
-- gives `NUDGE` points more and comes back, once each time the drag gets
-- there (Zeron's resize-edge bounce): `BOUNCE_MS` long, out in the first
-- `BOUNCE_OUT` of it and home in the rest, both smoothstep, so it starts
-- and lands still. With motion reduced, none.
M.NUDGE, M.BOUNCE_MS, M.BOUNCE_OUT = 5, 220, 0.32
-- (Smoothstep, 3t² − 2t³, is exactly this curve.)
M.SMOOTHSTEP = "cubicBezier(.3333,0,.6667,1)"
-- `anime.animate`'s params for a state of points: out to `toward` (-1 or
-- 1) times `NUDGE`, and back to 0. nil with motion reduced.
function M.bounce(toward: number): TableParams?
	if M.reduced_now() then
		return nil
	end
	return {
		value = {
			{ to = toward * M.NUDGE, duration = M.BOUNCE_MS * M.BOUNCE_OUT, ease = M.SMOOTHSTEP },
			{ to = 0, duration = M.BOUNCE_MS * (1 - M.BOUNCE_OUT), ease = M.SMOOTHSTEP },
		},
	} :: any
end

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


-- Tabs (ds/tabs, ds/tab_strip): ds/tabs' chosen wash gliding to the tab
-- that takes it, and a tab let go settling into its place, on one spring.
-- A little under critical damping: it lands in about a quarter of a second
-- with no overshoot to see, and a `ui.spring` sent somewhere new on the way
-- keeps its speed.
M.TAB_SPRING = { stiffness = 420, damping = 34 } :: SpringOptions
-- That spring as an anime ease, for a tab settling.
function M.tab_ease(): AnimeEase
	return anime.createSpring { stiffness = M.TAB_SPRING.stiffness, damping = M.TAB_SPRING.damping }
end
-- A tab picked up by a drag: this much larger, over this long, as it lifts
-- and as it's let go.
M.TAB_LIFT = 1.03
M.TAB_LIFT_TWEEN = { duration = 120, ease = "out(3)" } :: TweenOptions
-- The tabs beside one moving, opening or closing slide over on a stiffer
-- spring, critically damped: quick, and still, with nothing to wobble. (The
-- tab itself comes and goes at once: a chat alone's tab is replaced on each
-- navigation, too often for an entrance.) A strip's chosen wash follows its
-- tab on this one too, so the two never drift apart.
M.TAB_SLIDE_SPRING = { stiffness = 600, damping = 49 } :: SpringOptions
function M.tab_slide(): { [string]: any }?
	return M.layout { spring = anime.createSpring(M.TAB_SLIDE_SPRING) }
end
-- A tab carried near an end of its strip scrolls the strip under it, this
-- far from the edge, at this many points a second, till the end.
M.TAB_EDGE, M.TAB_EDGE_SPEED = 32, 700

return M

-- Segmented cells: progress as a row of discrete ticks, one per unit (a
-- todo's step, a test), so how far along reads at a glance. Done cells in
-- the done hue, failed in the failed hue, the current one in the working
-- hue, brightening and dimming while it works (still with motion reduced),
-- the rest faint.
--
--   Cells { done = 3, total = 8 }                      counts (current: the
--                                                      next after done)
--   Cells { done = fn, failed = fn, total = fn, current = false }
--   Cells { states = fn }                              each cell's state:
--                                                      "done" | "failed" |
--                                                      "current" | "pending"
--   Cells { …, fill = true }                           the cells stretch to
--                                                      the width given
--
-- `cell` (5) and `gap` (2) are points across, `h` (12) tall; `max` (48)
-- cells at most (more units share a cell). Every count may be a value or a
-- function of one.

local status = require("@self/status")
local motion = require("@self/motion")
local fade = require("@self/orch_spark").fade

export type State = "done" | "failed" | "current" | "pending"

export type Props = {
	done: (number | () -> number)?,
	failed: (number | () -> number)?,
	total: (number | () -> number)?,
	-- Whether the cell after the done ones is under way (true by default
	-- while some remain).
	current: (boolean | () -> boolean)?,
	states: (() -> { State })?,
	cell: number?,
	gap: number?,
	h: number?,
	max: number?,
	fill: boolean?,
	label: string?,
	-- Box props passed through.
	w: number?,
	grow: number?,
	shrink: number?,
	min_w: number?,
	id: string?,
	class: string?,
}

local INK: { [State]: string } = {
	done = "$zeron.status.done",
	failed = "$zeron.status.failed",
	current = "$zeron.status.working",
	pending = "$text.tertiary",
}
-- How strongly each state's ink is laid (pending: a faint tick).
local ALPHA: { [State]: number } = { done = 1, failed = 1, current = 1, pending = 0.28 }

local function read(v: any, default: any): any
	if type(v) == "function" then
		return (v :: any)()
	end
	if v == nil then
		return default
	end
	return v
end

-- Each cell's state, from the props.
local function cells_of(props: Props): { State }
	if props.states then
		return props.states()
	end
	local total = math.max(0, math.floor(read(props.total, 0)))
	local done = math.clamp(math.floor(read(props.done, 0)), 0, total)
	local failed = math.clamp(math.floor(read(props.failed, 0)), 0, total - done)
	local current = read(props.current, true)
	local max = props.max or 48
	-- (More units than cells: each cell a share of them.)
	local n = math.min(total, max)
	local per = if n > 0 then total / n else 1
	local out: { State } = {}
	for i = 1, n do
		local upto = i * per
		if upto <= done + 1e-6 then
			table.insert(out, "done" :: State)
		elseif upto <= done + failed + 1e-6 then
			table.insert(out, "failed" :: State)
		else
			table.insert(out, "pending" :: State)
		end
	end
	if current then
		for i = 1, n do
			if out[i] == "pending" then
				out[i] = "current"
				break
			end
		end
	end
	return out
end

-- The pulse over the current cell: its hue, brightening and dimming.
local function pulse(props: ContainerProps): Node
	if status.stepped() then
		-- (Off macOS, stepped on the status clock: ds/status `step`.)
		return status.Stepped(props, function(el: Element, now: number)
			anime.utils.set(el, { opacity = status.sine_at(now, 0.15, 0.85, 700) })
		end)
	end
	local playing = false
	props.on_visible = function(visible: boolean, el: Element)
		if visible then
			if not playing and not motion.reduced_now() then
				playing = true
				local anim = anime.animate(el, {
					opacity = { 0.15, 0.85 },
					duration = 700,
					ease = "inOutSine",
					loop = true,
					alternate = true,
					frameRate = 30,
				})
				status.in_phase(anim, 1400)
			end
		elseif playing then
			playing = false
			anime.utils.remove(el)
		end
	end
	return ui.div(props)
end

return ui.component(function(props: Props): Node
	local cw = props.cell or 5
	local gap = props.gap or 2
	local h = props.h or 12
	local width = ui.use_state(0)
	local cells = ui.computed(function(): { State }
		return cells_of(props)
	end)
	-- Where cell `i` starts and how wide it is, drawn `w` across.
	local function geometry(n: number, w: number): (number, number)
		if props.fill and n > 0 then
			local each = math.max(1, (w - gap * (n - 1)) / n)
			return each, gap
		end
		return cw, gap
	end
	local function current_at(): number?
		for i, s in cells:get() do
			if s == "current" then
				return i
			end
		end
		return nil
	end
	return ui.overlay {
		id = props.id,
		class = props.class or "orch-cells",
		relative = true,
		w = if props.fill then props.w else nil,
		w_full = props.fill and props.w == nil,
		grow = props.grow,
		shrink = props.shrink or 0,
		min_w = props.min_w,
		h = h,
		label = props.label,
		on_resize = function(size: { w: number, h: number })
			if math.abs(size.w - width:peek()) >= 1 then
				width:set(size.w)
			end
		end,
		ui.canvas {
			w = function(): number
				if props.fill then
					return math.max(0, width:get())
				end
				local n = #cells:get()
				return if n > 0 then n * cw + (n - 1) * gap else 0
			end,
			h = h,
			draw = function(c: Canvas, w: number, hh: number)
				local list = cells:get()
				local each, g = geometry(#list, w)
				for i, s in list do
					local x = (i - 1) * (each + g)
					c:rect(x, 0, each, hh, fade(INK[s], ALPHA[s]), math.min(1.5, each / 2))
				end
			end,
		},
		-- The current cell's glow, over it: laid in the flow of a row of its
		-- own (a spacer the width of the cells before it, then the glow), the
		-- overlay stacking that row on the canvas, so it sits on its cell
		-- whatever holds the cells (no absolute place to lose).
		ui.show(function(): boolean
			return current_at() ~= nil
		end, ui.row {
			h = h,
			pointer_events = "none",
			ui.div {
				shrink = 0,
				h = h,
				w = function(): number
					local i = current_at() or 1
					local each, g = geometry(#cells:get(), width:get())
					return (i - 1) * (each + g)
				end,
			},
			pulse {
				id = if props.id then `{props.id}-current` else nil,
				class = "orch-cells-current",
				shrink = 0,
				w = function(): number
					local each = geometry(#cells:get(), width:get())
					return each
				end,
				h = h,
				opacity = 0.35,
				rounded = ui.raw(1.5, "a cell's corner, as the canvas draws it"),
				bg = "$text.primary",
			},
		}),
	}
end)

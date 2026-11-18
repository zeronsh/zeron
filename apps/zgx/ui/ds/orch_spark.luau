-- A live sparkline: a thin bright line over the last few minutes, a
-- stipple of dots under it, a head at "now" with a soft halo, and what
-- happened along the way as thin upright marks (a test run starting, a
-- merge, a failure) in their hue. A small label over it with its live
-- figure at the end, a caption under it. Drawn on one canvas; it moves as
-- its samples come (no timer of its own).
--
--   Spark {
--     samples = function() return { { at = t, v = 0.42 }, … } end,  -- oldest first
--     span = 300,                       -- seconds shown (5 minutes)
--     lo = 0, hi = 1,                   -- the value's range
--     now = function() return t end,    -- the right edge (the last sample's)
--     events = function() return { { at = t, tone = "danger" }, … } end,
--     label = "CPU", value = function() return "42%" end,  -- or value_node
--     caption = function() return "5 min" end, caption_end = …,
--     height = 44,
--   }
--
-- An event's `tone`: "danger" (failed), "success", "working", "warning",
-- or "neutral". `Spark.fade(token, alpha)` is a theme token thinned, as a
-- canvas paints it.

local M = {}

export type Sample = { at: number, v: number }
export type Event = { at: number, tone: string?, label: string? }
type Text = string | Message | (() -> string)

export type Props = {
	samples: () -> { Sample },
	span: number?,
	lo: number?,
	hi: number?,
	now: (() -> number)?,
	events: (() -> { Event })?,
	label: Text?,
	value: Text?,
	-- A live figure of its own instead (an orch_counter).
	value_node: Node?,
	caption: Text?,
	caption_end: Text?,
	height: number?,
	ink: string?,
	-- What it says to assistive technology (the canvas's name).
	describe: (() -> string)?,
	id: string?,
	class: string?,
}

local TONE: { [string]: string } = {
	danger = "$zeron.status.failed",
	failed = "$zeron.status.failed",
	success = "$zeron.status.done",
	done = "$zeron.status.done",
	working = "$zeron.status.working",
	warning = "$warning",
	neutral = "$text.tertiary",
}

local function byte(x: number): number
	return math.clamp(math.floor(x * 255 + 0.5), 0, 255)
end

-- Faded paints made, by theme value and alpha.
local FADED: { [string]: string } = {}

-- `token` (`"$text.primary"`) at `alpha` of its own, as `#rrggbbaa` now
-- (tracked: read in a draw, a theme change paints again).
function M.fade(token: string, alpha: number): string
	ui.theme.current:get()
	local v = ui.theme.get(string.sub(token, 2))
	if type(v) ~= "string" or string.sub(v, 1, 1) ~= "#" or #v < 7 then
		return token
	end
	local key = `{v}/{alpha}`
	local kept = FADED[key]
	if kept then
		return kept
	end
	local a = if #v >= 9 then (tonumber(string.sub(v, 8, 9), 16) or 255) / 255 else 1
	local out = string.sub(v, 1, 7) .. string.format("%02x", byte(a * alpha))
	FADED[key] = out
	return out
end

local function small(t: Text?, color: string, tabular: boolean?): Node?
	if t == nil then
		return nil
	end
	return ui.text { t :: any, tabular = tabular, text_size = "$font.size.xs", color = color, truncate = true, min_w = 0 }
end

-- Where along the plot (0 to `w`) each moment falls: the window is `span`
-- seconds across. While the samples cover less than that, the line starts
-- at the left edge with the first of them and grows rightwards, its head
-- at the newest; once they fill it, the right edge is `now` (the newest,
-- unless given) and the oldest scroll off the left. Returns the function
-- placing a time, and whether the window is full.
function M.place(samples: { Sample }, span: number, w: number, now: number?): ((at: number) -> number, boolean)
	local first, last = samples[1], samples[#samples]
	local right = now or (if last then last.at else os.time())
	if first and right - first.at < span then
		local start = first.at
		return function(at: number): number
			return w * (at - start) / span
		end, false
	end
	return function(at: number): number
		return w * (1 - (right - at) / span)
	end, true
end

M.Spark = ui.component(function(props: Props): Node
	local h = props.height or 44
	local span = props.span or 300
	local lo, hi = props.lo or 0, props.hi or 1
	local ink = props.ink or "$text.primary"
	local label, caption, caption_end = props.label, props.caption, props.caption_end
	local value: Node? = props.value_node or small(props.value, "$text.secondary", true)

	local function draw(c: Canvas, w: number, hh: number)
		local samples = props.samples() or {}
		local events = if props.events then props.events() or {} else {}
		local top, bottom = 7, hh - 1.5
		local x_of = M.place(samples, span, w, if props.now then props.now() else nil)
		local function y_of(v: number): number
			local f = math.clamp((v - lo) / math.max(1e-9, hi - lo), 0, 1)
			return bottom - (bottom - top) * f
		end
		-- The plot's ground, and its axis along the foot.
		c:rect(0, 0, w, hh, M.fade("$text.primary", 0.035), 6)
		c:rect(0, hh - 1, w, 1, "$border.subtle")

		-- What happened, under the line.
		for _, e in events do
			local x = math.floor(x_of(e.at)) + 0.5
			if x >= 0 and x <= w then
				local paint = TONE[e.tone or "neutral"] or "$text.tertiary"
				c:rect(x - 0.5, 3, 1, hh - 4, M.fade(paint, 0.75))
				c:rect(x - 1.5, 1, 3, 3, paint, 0.5)
			end
		end

		-- The points in view (and the one just before, so the line enters).
		local pts: { { number } } = {}
		for i, s in samples do
			local x = x_of(s.at)
			local nxt = samples[i + 1]
			if x >= 0 or (nxt and x_of(nxt.at) >= 0) then
				table.insert(pts, { x, y_of(s.v) })
			end
		end
		if #pts == 0 then
			return
		end
		if #pts == 1 then
			-- (One sample yet: its head alone, at the left edge.)
			local x, y = math.max(pts[1][1], 2.5), pts[1][2]
			c:circle(x, y, 5.5, M.fade(ink, 0.14))
			c:circle(x, y, 2.25, ink)
			return
		end
		-- Smoothed through the midpoints.
		local line = ui.path():move(pts[1][1], pts[1][2])
		local area = ui.path():move(pts[1][1], bottom):line(pts[1][1], pts[1][2])
		for i = 2, #pts do
			local p, q = pts[i - 1], pts[i]
			local mx, my = (p[1] + q[1]) / 2, (p[2] + q[2]) / 2
			line = line:quad(p[1], p[2], mx, my)
			area = area:quad(p[1], p[2], mx, my)
		end
		local head = pts[#pts]
		line = line:line(head[1], head[2])
		area = area:line(head[1], head[2]):line(head[1], bottom):close()
		-- The stipple, under the line only.
		c:clip(area)
		local dot = M.fade(ink, 0.32)
		local x0 = math.max(0, math.floor(pts[1][1]))
		local x = x0 + 1.5
		while x < head[1] do
			local y = bottom - 2.5
			while y > top - 2 do
				c:rect(x, y, 1, 1, dot)
				y -= 3
			end
			x += 3
		end
		c:restore()
		c:stroke(line, ink, 1.4, { join = "round", cap = "round" })
		-- The head, now: a soft halo and its dot.
		c:circle(head[1], head[2], 6, M.fade(ink, 0.12))
		c:circle(head[1], head[2], 3.5, M.fade(ink, 0.22))
		c:circle(head[1], head[2], 2.1, ink)
	end

	return ui.column {
		id = props.id,
		class = props.class or "orch-spark",
		w_full = true,
		gap = "$space.1",
		if label or value
			then ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				small(label, "$text.tertiary"),
				ui.div { grow = 1 },
				value,
			}
			else nil,
		ui.canvas {
			class = "orch-spark-plot",
			w_full = true,
			h = h,
			label = props.describe,
			draw = draw,
		},
		if caption or caption_end
			then ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				small(caption, "$text.tertiary", true),
				ui.div { grow = 1 },
				small(caption_end, "$text.tertiary", true),
			}
			else nil,
	}
end)

return M

-- Small parts the Map's views share: a todo as segmented cells, a live
-- line with a stippled fill, a number, and the 300 ms a state is shown at
-- least. (Stand-ins until ds/orch_cells, orch_spark, orch_counter and
-- orch_hold land; same jobs, same props where they overlap.)
--
--   parts.Cells { done, total, failed?, current?, w?, h? }
--   parts.Spark { values, w?, h, ink? }
--   parts.Counter { value, suffix?, text_size?, color?, weight? }
--   parts.Hold(ms?)    a holder: `holder(id, sig, value)` gives what to
--                      show for `id` now (the last value if `sig` changed
--                      under `ms` ago), and `holder.wake` a state that ticks
--                      when a held one may show its latest

local kit = require("@self/orch_map_kit")

local M = {}

local function read(v: any): any
	if type(v) == "function" then
		return (v :: any)()
	end
	return v
end

-- A todo as cells: one a step, done in the done ink, the current one
-- brighter, failed in danger, the rest faint.
function M.Cells(props: { done: any, total: any, failed: any?, current: any?, w: number?, h: number?, cell: number? }): Node
	local h = props.h or 10
	local cell = props.cell or 3
	local gap = 2
	return ui.canvas {
		class = "orch-cells",
		w = function(): number
			if props.w then
				return props.w
			end
			local total = math.min(read(props.total) or 0, 24)
			return math.max(0, total * (cell + gap) - gap)
		end,
		h = h,
		shrink = 0,
		label = function(): string
			return `{read(props.done) or 0}/{read(props.total) or 0}`
		end,
		draw = function(c: any, w: number, hh: number)
			local total = read(props.total) or 0
			if total <= 0 then
				return
			end
			local done = math.min(read(props.done) or 0, total)
			local failed = read(props.failed) or 0
			local n = math.min(total, 24)
			local step = (w + gap) / n
			local cw = math.max(1.5, step - gap)
			local scale = n / total
			for i = 0, n - 1 do
				local x = i * step
				local at = (i + 1) / scale
				local ink
				if at <= done - failed then
					ink = kit.ink("zeron.status.done", 0.85)
				elseif at <= done then
					ink = kit.ink("zeron.status.failed", 0.9)
				elseif i == math.floor(done * scale) and read(props.current) then
					ink = kit.ink("text.primary", 0.7)
				else
					ink = kit.ink("text.primary", 0.14)
				end
				c:rect(x, 0, cw, hh, ink, 1)
			end
		end,
	}
end

-- A live line: thin, bright, a stippled fill under it, its head a dot with
-- a halo.
function M.Spark(props: { values: () -> { number }, w: number?, h: number, ink: string? }): Node
	return ui.canvas {
		class = "orch-spark",
		w = props.w,
		w_full = props.w == nil,
		h = props.h,
		shrink = 0,
		draw = function(c: any, w: number, h: number)
			local vs = props.values() or {}
			local n = #vs
			local token = props.ink or "text.primary"
			c:line(0, h - 0.5, w, h - 0.5, kit.ink("text.primary", 0.08), 1)
			if n < 2 then
				return
			end
			local pad = 4
			-- (Scaled to its own range, a little room round it: the number
			-- beside it says how much; the line, how it moves.)
			local low, high = 1, 0
			for _, v in vs do
				low = math.min(low, v)
				high = math.max(high, v)
			end
			local lo = math.max(0, low - 0.08)
			local range = math.max(0.2, math.min(1, high + 0.08) - lo)
			local function norm(v: number): number
				return math.clamp((v - lo) / range, 0, 1)
			end
			local function at(i: number): (number, number)
				local x = (i - 1) / (n - 1) * (w - pad)
				local y = h - 1 - norm(vs[i]) * (h - 6)
				return x, y
			end
			-- The stipple: a dot every few points under the line.
			local dots = kit.ink(token, 0.2)
			local step = 3
			for x = 1, w - pad, step do
				local f = x / (w - pad) * (n - 1) + 1
				local i = math.floor(f)
				local t = f - i
				local v = (vs[i] or 0) * (1 - t) + (vs[math.min(n, i + 1)] or 0) * t
				local top = h - 1 - norm(v) * (h - 6)
				local y = h - 2
				while y > top + 2 do
					c:rect(x, y, 1, 1, dots)
					y -= step
				end
			end
			local path = ui.path()
			for i = 1, n do
				local x, y = at(i)
				path = if i == 1 then path:move(x, y) else path:line(x, y)
			end
			c:stroke(path, kit.ink(token, 0.85), 1.25, { join = "round", cap = "round" })
			local hx, hy = at(n)
			c:circle(hx, hy, 5, kit.ink(token, 0.16))
			c:circle(hx, hy, 2.25, kit.ink(token, 1))
		end,
	}
end

-- A number, in tabular figures.
function M.Counter(props: { value: () -> any, suffix: string?, text_size: string?, color: any, weight: string? }): Node
	return ui.text {
		function(): string
			local v = props.value()
			if v == nil then
				return "–"
			end
			return `{v}{props.suffix or ""}`
		end,
		tabular = true,
		shrink = 0,
		text_size = props.text_size or "$font.size.sm",
		weight = props.weight,
		color = props.color or "$text.primary",
	}
end

-- What to show of something that changes, each state shown at least `ms`.
export type Holder = typeof(setmetatable({} :: { wake: State<number> }, {} :: { __call: (any, string, string, any) -> any }))
function M.Hold(ms: number?): any
	local min = ms or 300
	local shown: { [string]: { sig: string, value: any, at: number } } = {}
	local wake = ui.use_state(0)
	local pending = false
	local holder = { wake = wake }
	return setmetatable(holder, {
		__call = function(_self: any, id: string, sig: string, value: any): any
			local now = ui.now()
			local s = shown[id]
			if not s or s.sig == sig then
				shown[id] = { sig = sig, value = value, at = if s then s.at else now }
				return value
			end
			local age = now - s.at
			if age >= min then
				shown[id] = { sig = sig, value = value, at = now }
				return value
			end
			if not pending then
				pending = true
				ui.after(math.ceil(min - age) + 1, function()
					pending = false
					wake:set(wake:peek() + 1)
				end)
			end
			return s.value
		end,
	})
end

return M

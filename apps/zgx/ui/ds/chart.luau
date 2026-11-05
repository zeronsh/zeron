-- A chart laid straight on the page: bars (grouped, stacked or across),
-- lines, areas and scatter, drawn on one canvas in theme colours. Quiet
-- structure: gridlines a hairline at each round tick, the zero line firmer,
-- tick labels small in the tertiary ink; the data is what reads. Under the
-- pointer a category is picked out (a wash behind its bars, a hairline
-- through a line's points) and the legend says its values, so nothing
-- floats over the plot.
--
--   Chart {
--     data = function() return {
--       type = "bar" | "line" | "area" | "scatter",
--       x = { "Jan", "Feb" },                    -- categories
--       series = { { name, color, values = { 3, false, 5 }, points = { x1, y1, … } } },
--       stacked, horizontal, legend,
--       yaxis = { lo, hi, ticks, labels },       -- round ticks and their labels
--       xaxis = { … },                           -- a scatter's
--       fmt = { prefix, unit, decimals },        -- how values read
--     } end,
--     height = 220,
--   }
--
-- (`false` is a gap in a series. A colour is a visual's colour word or
-- `{ token, mix, t, alpha }`: ds/visual_ink. ds/visual gives its charts'
-- axes from the host, src/visuals.rs.)

local ink = require("@self/visual_ink")

export type Props = {
	data: () -> any,
	height: number,
	label: string?,
}

-- The plot's insets: room for the tick labels.
local PAD_TOP, PAD_RIGHT, LABEL_GAP, X_LABELS = 8, 6, 8, 22
local DOT = 8

local function fade(color: any, alpha: number): any
	if type(color) == "table" then
		local c = table.clone(color)
		c.alpha = (c.alpha or 1) * alpha
		return c
	end
	return { token = color or "chart1", alpha = alpha }
end

local function n(token: string, fallback: number): number
	local v = ui.theme.get(token)
	return if type(v) == "number" then v else fallback
end

local function value_at(s: any, i: number): number?
	local v = s.values and s.values[i]
	return if type(v) == "number" then v else nil
end

return ui.component(function(props: Props): Node
	local hovered = ui.use_state(0)
	-- Where the last drawing put things, for the pointer.
	local geometry = { left = 0, top = 0, w = 0, h = 0, n = 0, horizontal = false }

	local function data(): any
		return props.data() or {}
	end

	local function draw(c: any, w: number, h: number)
		local d = data()
		local series = d.series or {}
		local kind = d.type or "bar"
		local y = d.yaxis or { lo = 0, hi = 1, ticks = {}, labels = {} }
		local horizontal = d.horizontal == true
		local cats = d.x or {}
		local count = #cats
		for _, s in series do
			count = math.max(count, #(s.values or {}))
		end
		local size = n("zeron.visual.text.xs.size", 12)
		local faint, grid, axis = "$text.tertiary", "$border.subtle", "$border"
		-- The insets: the widest tick label on the left (a horizontal
		-- chart's, its widest category), the categories' line underneath.
		local left_labels = if horizontal then cats else (y.labels or {})
		local left = 0
		for _, label in left_labels do
			local lw = c:measure_text(tostring(label), { size = size })
			left = math.max(left, lw)
		end
		left = math.min(left, w * 0.4) + LABEL_GAP
		local top, bottom = PAD_TOP, X_LABELS
		local pw, ph = math.max(1, w - left - PAD_RIGHT), math.max(1, h - top - bottom)
		geometry.left, geometry.top, geometry.w, geometry.h, geometry.n = left, top, pw, ph, count
		geometry.horizontal = horizontal
		geometry.scatter = kind == "scatter"
		local lo, hi = y.lo or 0, y.hi or 1
		local span = if hi - lo == 0 then 1 else hi - lo
		-- A value's place along the value axis.
		local function vy(v: number): number
			return top + ph - (v - lo) / span * ph
		end
		local function vx(v: number): number
			return left + (v - lo) / span * pw
		end
		local hover = hovered:get()

		-- Gridlines and their labels, along the value axis.
		for i, t in y.ticks or {} do
			local label = (y.labels or {})[i] or ""
			local line = if t == 0 then axis else grid
			if horizontal then
				local x = vx(t)
				c:rect(x - 0.5, top, 1, ph, line)
				c:text(label, x, top + ph + 6, { size = size, color = faint, align = "center", baseline = "top", tabular = true })
			else
				local yy = vy(t)
				c:rect(left, yy - 0.5, pw, 1, line)
				c:text(label, left - LABEL_GAP, yy, { size = size, color = faint, align = "end", baseline = "middle", tabular = true })
			end
		end

		if kind == "scatter" then
			local xa = d.xaxis or { lo = 0, hi = 1, ticks = {}, labels = {} }
			local xlo, xhi = xa.lo or 0, xa.hi or 1
			local xspan = if xhi - xlo == 0 then 1 else xhi - xlo
			local function sx(v: number): number
				return left + (v - xlo) / xspan * pw
			end
			for i, t in xa.ticks or {} do
				local x = sx(t)
				c:rect(x - 0.5, top, 1, ph, if t == 0 then axis else grid)
				c:text((xa.labels or {})[i] or "", x, top + ph + 6, { size = size, color = faint, align = "center", baseline = "top", tabular = true })
			end
			for si, s in series do
				local color = ink.canvas(s.color or `chart{si}`)
				local pts = s.points or {}
				for i = 1, #pts - 1, 2 do
					c:circle(sx(pts[i]), vy(pts[i + 1]), 3.5, color)
				end
			end
			return
		end

		if count == 0 then
			return
		end
		local band = (if horizontal then ph else pw) / count
		-- Category labels: as many as fit, evenly skipped.
		local widest = 0
		for _, label in cats do
			widest = math.max(widest, (c:measure_text(tostring(label), { size = size })))
		end
		if horizontal then
			for i, label in cats do
				local yy = top + (i - 0.5) * band
				c:text(tostring(label), left - LABEL_GAP, yy, { size = size, color = if hover == i then "$text.primary" else faint, align = "end", baseline = "middle" })
			end
		else
			local every = math.max(1, math.ceil((widest + 12) / band))
			for i, label in cats do
				if (i - 1) % every == 0 then
					c:text(tostring(label), left + (i - 0.5) * band, top + ph + 6, {
						size = size,
						color = if hover == i then "$text.primary" else faint,
						align = "center",
						baseline = "top",
					})
				end
			end
		end

		if kind == "bar" then
			local stacked = d.stacked == true
			local group = band * (if count > 24 then 0.86 else 0.68)
			local per = if stacked then group else group / math.max(1, #series)
			local gap = if stacked or #series < 2 then 0 else math.min(2, per * 0.15)
			local radius = math.min(3, per / 4)
			if hover > 0 and hover <= count then
				if horizontal then
					c:rect(left, top + (hover - 1) * band, pw, band, "$hover", 4)
				else
					c:rect(left + (hover - 1) * band, top, band, ph, "$hover", 4)
				end
			end
			for i = 1, count do
				local up, down = 0, 0
				for si, s in series do
					local v = value_at(s, i)
					if v then
						local a, b
						if stacked then
							if v >= 0 then
								a, b = up, up + v
								up += v
							else
								a, b = down + v, down
								down += v
							end
						else
							a, b = math.min(0, v), math.max(0, v)
						end
						local color = ink.canvas(s.color or `chart{si}`)
						local offset = if stacked then 0 else (si - 1) * per
						if horizontal then
							local y0 = top + (i - 1) * band + (band - group) / 2 + offset + gap / 2
							local x0, x1 = vx(math.max(a, lo)), vx(math.min(b, hi))
							c:rect(x0, y0, math.max(0, x1 - x0), math.max(1, per - gap), color, radius)
						else
							local x0 = left + (i - 1) * band + (band - group) / 2 + offset + gap / 2
							local y0, y1 = vy(math.min(b, hi)), vy(math.max(a, lo))
							c:rect(x0, y0, math.max(1, per - gap), math.max(0, y1 - y0), color, radius)
						end
					end
				end
			end
			return
		end

		-- Lines and areas: each series through its categories' middles, a
		-- gap breaking the line; a stacked area on the ones under it.
		local function cx(i: number): number
			return left + (i - 0.5) * band
		end
		local base: { number } = {}
		for i = 1, count do
			base[i] = 0
		end
		if hover > 0 and hover <= count then
			c:rect(cx(hover) - 0.5, top, 1, ph, "$border.strong")
		end
		local stacked = d.stacked == true and kind == "area"
		for si, s in series do
			local color = s.color or `chart{si}`
			local paint = ink.canvas(color)
			local tops: { number? } = {}
			for i = 1, count do
				local v = value_at(s, i)
				if v then
					tops[i] = if stacked then base[i] + v else v
				end
			end
			-- Runs of values, between gaps.
			local runs: { { number } } = {}
			local run: { number }? = nil
			for i = 1, count do
				if tops[i] then
					if not run then
						run = {}
						table.insert(runs, run :: { number })
					end
					table.insert(run :: { number }, i)
				else
					run = nil
				end
			end
			for _, r in runs do
				if kind == "area" then
					local p = ui.path():move(cx(r[1]), vy(tops[r[1]] :: number))
					for k = 2, #r do
						p = p:line(cx(r[k]), vy(tops[r[k]] :: number))
					end
					for k = #r, 1, -1 do
						local under = if stacked then base[r[k]] else math.max(lo, math.min(0, hi))
						p = p:line(cx(r[k]), vy(under))
					end
					p = p:close()
					c:fill(p, ui.linear(0, top, 0, top + ph, { ink.hex(fade(color, 0.32)), ink.hex(fade(color, 0.04)) }))
				end
				if #r > 1 then
					local p = ui.path():move(cx(r[1]), vy(tops[r[1]] :: number))
					for k = 2, #r do
						p = p:line(cx(r[k]), vy(tops[r[k]] :: number))
					end
					c:stroke(p, paint, 2, { join = "round", cap = "round" })
				end
				-- Points where there are few enough to tell apart, or one alone.
				if #r == 1 or (band >= 14 and kind == "line") then
					for _, i in r do
						c:circle(cx(i), vy(tops[i] :: number), if hover == i then 4 else 2.5, paint)
					end
				elseif hover > 0 and tops[hover] then
					c:circle(cx(hover), vy(tops[hover] :: number), 4, paint)
				end
			end
			if stacked then
				for i = 1, count do
					if tops[i] then
						base[i] = tops[i] :: number
					end
				end
			end
		end
	end

	local function pointer(e: any)
		local g = geometry
		if g.n <= 0 or g.scatter then
			return
		end
		local at = if g.horizontal then (e.local_y - g.top) / g.h else (e.local_x - g.left) / g.w
		local i = math.floor(at * g.n) + 1
		hovered:set(if at >= 0 and at < 1 then i else 0)
	end

	-- The legend: each series' mark and name, and under the pointer its
	-- value there, after the category's name.
	local function legend(): boolean
		local d = data()
		if d.legend == false then
			return false
		end
		local named = false
		for _, s in d.series or {} do
			if s.name and s.name ~= "" then
				named = true
			end
		end
		return named and (#(d.series or {}) > 1 or hovered:get() > 0)
	end
	local function category(): string
		local i = hovered:get()
		local d = data()
		return if i > 0 and d.x and d.x[i] then tostring(d.x[i]) else ""
	end
	local names = ui.computed(function(): { number }
		local out = {}
		for i in data().series or {} do
			table.insert(out, i)
		end
		return out
	end)
	local text_sm = { size = "$zeron.visual.text.sm.size", line = "$zeron.visual.text.sm.line" }
	return ui.column {
		class = "chart",
		w_full = true,
		gap = "$space.2",
		ui.show(
			legend,
			ui.row {
				class = "chart-legend",
				flex_wrap = true,
				items = "center",
				gap = "$space.4",
				ui.show(
					function(): boolean
						return category() ~= ""
					end,
					ui.text {
						category,
						literal = true,
						text_size = text_sm.size,
						line_height = text_sm.line,
						weight = "$font.weight.medium",
						color = "$text.primary",
					}
				),
				ui.each(names, function(si: number): Node
					local function s(): any
						return (data().series or {})[si] or {}
					end
					local function shown(): string
						local i = hovered:get()
						local v = if i > 0 then value_at(s(), i) else nil
						return if v then ink.format(v, data().fmt) else ""
					end
					return ui.row {
						items = "center",
						gap = "$zeron.space.tight",
						ui.div {
							w = DOT,
							h = DOT,
							shrink = 0,
							rounded = "$radius.full",
							bg = function(): any
								return ink.paint(s().color or `chart{si}`)
							end,
						},
						ui.text {
							function(): string
								return s().name or ""
							end,
							literal = true,
							text_size = text_sm.size,
							line_height = text_sm.line,
							color = "$text.secondary",
						},
						ui.show(
							function(): boolean
								return shown() ~= ""
							end,
							ui.text {
								shown,
								literal = true,
								tabular = true,
								text_size = text_sm.size,
								line_height = text_sm.line,
								weight = "$font.weight.medium",
								color = "$text.primary",
							}
						),
					}
				end, { flex_wrap = true, items = "center", gap_x = "$space.4", gap_y = "$space.1" }),
			}
		),
		ui.canvas {
			class = "chart-plot",
			w_full = true,
			h = props.height,
			label = props.label,
			draw = draw,
			on_pointer_move = pointer,
			on_hover = function(inside: boolean)
				if not inside then
					hovered:set(0)
				end
			end,
		},
	}
end)

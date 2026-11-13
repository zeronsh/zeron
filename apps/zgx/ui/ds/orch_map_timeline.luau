-- The Map's Timeline: one lane a thread, under its machine and project,
-- over the last 15 minutes or hour. A faint track; where it worked, a bar
-- whose texture says what at (thinking stippled, testing in cells,
-- building solid, editing lighter); waiting on you, an open bar in input's
-- ink; ticks where something happened (a report, an ask, a failure, a
-- merge); at "now" a head on what still works, and at the end of the lane
-- what it does now. A click opens the thread.
--
--   Timeline {
--     world, chat, comms,              as ds/orch_map's
--     timeline = function() return orchestration.timeline:get() end,
--     window = function() return 15 or 60 end,
--     on_window = function(minutes) end,
--     on_open_chat, on_open_orchestrator,
--   }
--   (Its rows and what each lane draws: ds/orch_map_spans, pure.)

local Icon = require("@self/icon")
local status = require("@self/status")
local locale = require("@self/locale")
local hue = require("@self/orch_hue")
local kit = require("@self/orch_map_kit")
local parts = require("@self/orch_map_parts")
local model = require("@self/orch_map_model")

local TOP_H = 34
local AXIS_H = 34
local BAR = 4

local M = {}

local spans = require("@self/orch_map_spans")
type Row = spans.Row
type Built = spans.Built
local MACHINE_H, PROJECT_H, LANE_H = spans.MACHINE_H, spans.PROJECT_H, spans.LANE_H
M.spans_of = spans.spans_of
M.build = spans.build

-- The states the bar draws open: waiting on you.
local WAITING: { [string]: boolean } = { input = true, approval = true }

-- How a working span's bar is drawn, by what it did: its texture and ink.
local TEXTURE: { [string]: string } = {
	thinking = "stipple",
	testing = "cells",
	building = "solid",
	installing = "solid",
	merging = "solid",
	pushing = "solid",
	editing = "mid",
	running = "mid",
	delegating = "mid",
	reading = "light",
	reviewing = "light",
	browsing = "light",
}
M.TEXTURE = TEXTURE

local TICK_INK: { [string]: string } = {
	report = "text.primary",
	ask = "zeron.status.input",
	failed = "zeron.status.failed",
	merged = "zeron.status.done",
	done = "zeron.status.done",
}

local function bar(c: any, x0: number, x1: number, mid: number, texture: string)
	local w = x1 - x0
	if w <= 0 then
		return
	end
	local top = mid - BAR / 2
	if texture == "stipple" then
		local ink = kit.ink("text.primary", 0.5)
		local x = x0
		while x < x1 - 0.5 do
			c:rect(x, top - 0.5, 1.25, 1.25, ink)
			c:rect(x + 1.5, top + 2.5, 1.25, 1.25, ink)
			x += 3
		end
	elseif texture == "cells" then
		local ink = kit.ink("text.primary", 0.7)
		local x = x0
		while x < x1 - 0.5 do
			c:rect(x, top - 2, math.min(1.5, x1 - x), BAR + 4, ink, 0.5)
			x += 3.5
		end
	else
		local alpha = if texture == "solid" then 0.72 elseif texture == "mid" then 0.36 else 0.2
		c:rect(x0, top, math.max(1.5, w), BAR, kit.ink("text.primary", alpha), math.min(BAR / 2, w / 2))
	end
end

local function tick(c: any, x: number, mid: number, kind: string)
	local ink = kit.ink(TICK_INK[kind] or "text.primary", if kind == "report" then 0.6 else 1)
	c:line(x, mid - 8, x, mid + 6, ink, 1.25)
	c:rect(x - 1.5, mid - 10, 3, 3, ink, 0.5)
end

-- The legend's entries: the textures, then the ticks.
local LEGEND = {
	{ kind = "thinking", texture = "stipple" },
	{ kind = "editing", texture = "mid" },
	{ kind = "testing", texture = "cells" },
	{ kind = "building", texture = "solid" },
}
local LEGEND_TICKS = { "report", "ask", "failed", "merged" }

M.View = ui.component(function(props: {
	world: () -> any,
	chat: (id: string) -> any,
	comms: () -> { any },
	timeline: () -> { [string]: { any } },
	window: () -> number,
	on_window: (minutes: number) -> (),
	on_open_chat: (id: string) -> (),
	on_open_orchestrator: (id: string) -> (),
}): Node
	local width = ui.use_state(720)
	-- (The lanes slide on as time passes: a look every few seconds, a
	-- point at the hour's scale.)
	local clock = ui.use_state(os.time())
	ui.every(5000, function()
		clock:set(os.time())
	end)
	local built = ui.computed(function(): Built
		return M.build {
			world = props.world(),
			chat = props.chat,
			timeline = props.timeline() or {},
			comms = props.comms() or {},
			now = clock:get(),
			window = props.window(),
		}
	end)
	local rows = ui.computed(function(): { Row }
		return built:get().rows
	end)
	-- Where the chart runs: after the labels, before what each does now.
	local geom = ui.computed(function(): { label: number, x0: number, x1: number, tail: number, w: number }
		local w = width:get()
		local label = math.clamp(math.floor(w * 0.24), 132, 230)
		local tail = if w >= 760 then math.clamp(math.floor(w * 0.2), 150, 240) elseif w >= 560 then 110 else 0
		local x0 = label + 12
		local x1 = w - tail - (if tail > 0 then 16 else 10)
		return { label = label, x0 = x0, x1 = math.max(x0 + 40, x1), tail = tail, w = w }
	end)
	local function x_of(t: number): number
		local b, g = built:get(), geom:get()
		return g.x0 + (t - b.t0) / math.max(1, b.t1 - b.t0) * (g.x1 - g.x0)
	end

	local function draw(c: any, _w: number, _h: number)
		local b, g = built:get(), geom:get()
		local x0, x1 = g.x0, g.x1
		local faint = kit.ink("text.primary", 0.05)
		-- The minutes: a hairline down at each mark, the axis under them.
		local step = if (b.t1 - b.t0) > 1800 then 300 else 60
		local major = if step == 300 then 900 else 300
		local axis_y = b.h + 6
		c:line(x0, axis_y, x1, axis_y, kit.ink("text.primary", 0.12), 1)
		local t = b.t1
		while t >= b.t0 - 0.5 do
			local x = x_of(t)
			local ago_s = b.t1 - t
			local is_major = ago_s % major == 0
			if is_major and ago_s > 0 then
				c:line(x, 0, x, b.h, faint, 1)
			end
			c:line(x, axis_y, x, axis_y + (if is_major then 5 else 3), kit.ink("text.primary", if is_major then 0.3 else 0.16), 1)
			if is_major then
				local label = if ago_s == 0 then locale.s("ds.orch.map.now") else `−{ago_s // 60}m`
				c:text(label, x, axis_y + 8, { size = 10, color = kit.ink("text.primary", 0.42), align = if ago_s == 0 then "end" else "center", baseline = "top", tabular = true })
			end
			t -= step
		end
		-- Now: a dotted line down every lane.
		c:line(x1, 0, x1, b.h, kit.ink("text.primary", 0.14), 1, { dash = { 1, 3 } })

		for _, row in b.rows do
			if row.kind ~= "lane" then
				continue
			end
			local mid = row.y + row.h / 2
			local spans = row.spans or {}
			local start = if #spans > 0 then x_of(spans[1].t0) else x0
			c:line(start, mid, x1, mid, kit.ink("text.primary", if #spans > 0 then 0.1 else 0.05), 1)
			for si, s in spans do
				local a, z = x_of(s.t0), x_of(s.t1)
				-- (A breath between one span and the next: each change shows.)
				if spans[si + 1] and z - a > 4 then
					z -= 2
				end
				if s.status == "working" then
					bar(c, a, z, mid, TEXTURE[s.kind or ""] or "mid")
				elseif WAITING[s.status] then
					local ink = kit.ink(`zeron.status.{s.status}`, 0.9)
					local path = ui.path():rect(a + 0.5, mid - BAR / 2 + 0.5, math.max(1, z - a - 1), BAR - 1, 2.5)
					c:stroke(path, ink, 1)
				elseif s.status == "failed" then
					c:line(a, mid, z, mid, kit.ink("zeron.status.failed", 0.45), 1.5)
				end
			end
			for _, k in row.ticks or {} do
				tick(c, x_of(k.t), mid, k.kind)
			end
			-- The head: what still works, at now.
			local last = spans[#spans]
			if last and last.t1 >= b.t1 - 1 then
				if last.status == "working" then
					c:circle(x1, mid, 6.5, kit.ink("text.primary", 0.13))
					c:circle(x1, mid, 3, kit.ink("text.primary", 0.95))
				elseif WAITING[last.status] then
					c:circle(x1, mid, 6.5, kit.ink(`zeron.status.{last.status}`, 0.18))
					c:circle(x1, mid, 3, kit.ink(`zeron.status.{last.status}`, 1))
				end
			end
		end
	end

	local function machine_row(row: State<any>): Node
		local function load(): any
			return ((props.world().loads or {}) :: any)[row:get().machine]
		end
		return ui.row {
			class = "orch-tl-machine",
			absolute = true,
			left = 0,
			right = 0,
			top = function(): number
				return row:get().y
			end,
			h = MACHINE_H,
			gap = "$space.2",
			items = "center",
			ui.show(function(): boolean
				return row:get().y > 0
			end, ui.div { absolute = true, left = 0, right = 0, top = 0, h = 1, bg = "$border.subtle" }),
			ui.div { color = "$text.tertiary", shrink = 0, Icon { name = function(): any
				return kit.platform_icon(row:get().device)
			end, size = 14 } },
			ui.text {
				function(): string
					return row:get().name or ""
				end,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.primary",
				shrink = 0,
			},
			ui.text {
				function(): string
					local d = row:get().device
					local word = kit.platform_word(d)
					local here = d and (d.here == true or d.id == props.world().here)
					return if here then `{word} · {locale.s("ds.orch.map.here")}` else word
				end,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
				truncate = true,
				min_w = 0,
			},
			ui.div { grow = 1 },
			parts.Counter {
				value = function(): any
					local l = load()
					return if l and l.cpu then `{math.floor(l.cpu * 100 + 0.5)}%` else nil
				end,
				text_size = "$font.size.xs",
				color = "$text.secondary",
			},
		}
	end

	local function project_row(row: State<any>): Node
		return ui.row {
			class = "orch-tl-project",
			absolute = true,
			left = 0,
			top = function(): number
				return row:get().y
			end,
			h = PROJECT_H,
			pl = 22,
			gap = "$space.1",
			items = "center",
			ui.div { color = "$text.tertiary", shrink = 0, Icon { name = "folder", size = 12 } },
			ui.text {
				function(): string
					return row:get().name or ""
				end,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
		}
	end

	local function lane_row(row: State<any>): Node
		local function t(): any
			return row:get().thread or {}
		end
		local function c(): any
			return props.chat(row:get().id) or {}
		end
		local function st(): string
			return model.status_of(c())
		end
		local function activity(): any
			local a = c().activity
			return if st() == "working" and a then a else nil
		end
		return ui.button {
			class = if (row:peek().thread or {}).kind == "crew" then "orch-tl-lane orch-pill" else "orch-tl-lane",
			label = function(): string
				local a = activity()
				local title = t().title or ""
				return if a then `{title}, {locale.s(`ds.orch.activity.{a.kind}`)}` else title
			end,
			absolute = true,
			left = 0,
			right = 0,
			top = function(): number
				return row:get().y
			end,
			h = LANE_H,
			rounded = "$radius.sm",
			bg = "#00000000",
			hover = { bg = "$hover" },
			on_click = function()
				local th = row:peek().thread
				if th and th.kind == "orchestrator" then
					props.on_open_orchestrator(row:peek().id)
				else
					props.on_open_chat(row:peek().id)
				end
			end,
			-- Its name, where the lanes' names are.
			ui.row {
				absolute = true,
				left = 22,
				top = 0,
				h = LANE_H,
				w = function(): number
					return geom:get().label - 22
				end,
				gap = 6,
				items = "center",
				ui.div {
					w = 12,
					h = 12,
					shrink = 0,
					items = "center",
					justify = "center",
					ui.show(function(): boolean
						return st() ~= "idle"
					end, status.StatusIcon { kind = function(): any
						return st()
					end, size = 12 }),
				},
				-- (Its orchestrator's dot, or the room for one: the titles align.)
				ui.div {
					w = 6,
					h = 6,
					shrink = 0,
					ui.show(function(): boolean
						return t().hue ~= nil
					end, hue.Dot { hue = function(): number?
						return t().hue
					end, size = 6 }),
				},
				ui.text {
					function(): string
						return t().title or ""
					end,
					min_w = 0,
					truncate = true,
					text_size = "$font.size.xs",
					weight = function(): string
						return if t().kind == "orchestrator" then "$font.weight.semibold" else "$font.weight.regular"
					end,
					color = function(): string
						local s = st()
						return if s == "done" or s == "idle" then "$text.secondary" else "$text.primary"
					end,
				},
			},
			-- What it does now, at the lane's end.
			ui.show(function(): boolean
				return geom:get().tail > 0
			end, ui.row {
				absolute = true,
				top = 0,
				h = LANE_H,
				left = function(): number
					return geom:get().x1 + 14
				end,
				right = 4,
				gap = 6,
				items = "center",
				ui.text {
					function(): string
						local a = activity()
						local s = st()
						if a then
							return locale.s(`ds.orch.activity.{a.kind}`)
						elseif s == "input" or s == "approval" or s == "failed" then
							return status.KINDS[s :: any].long
						elseif s == "done" then
							local u = c().updated
							return if u then `{status.KINDS.done.label} · {status.ago(kit.since(u))}` else status.KINDS.done.label
						end
						return ""
					end,
					shrink = 0,
					text_size = "$font.size.xs",
					color = function(): string
						local s = st()
						if s == "input" or s == "approval" or s == "failed" then
							return `$zeron.status.{s}`
						end
						return if s == "done" then "$text.tertiary" else "$text.secondary"
					end,
				},
				ui.text {
					function(): string
						local a = activity()
						return if a then a.label or "" else ""
					end,
					literal = true,
					grow = 1,
					min_w = 0,
					truncate = true,
					font = "$font.mono",
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			}),
		}
	end

	local function any_row(r: Row, _i: number, row: State<any>): Node
		if r.kind == "machine" then
			return machine_row(row)
		elseif r.kind == "project" then
			return project_row(row)
		end
		return lane_row(row)
	end

	local function swatch(texture: string?, tick_kind: string?): Node
		return ui.canvas {
			w = if texture then 18 else 6,
			h = 14,
			shrink = 0,
			draw = function(c: any, w: number, h: number)
				if texture then
					bar(c, 0, w, h / 2, texture)
				else
					tick(c, w / 2, h / 2 + 2, tick_kind :: string)
				end
			end,
		}
	end
	local function legend_item(word: any, texture: string?, tick_kind: string?): Node
		return ui.row {
			gap = 5,
			items = "center",
			shrink = 0,
			swatch(texture, tick_kind),
			ui.text { word, text_size = "$font.size.xs", color = "$text.tertiary" },
		}
	end
	local legend: any = { gap = "$space.3", items = "center", shrink = 1, min_w = 0, overflow = "hidden" }
	for _, e in LEGEND do
		table.insert(legend, legend_item(locale.t(`ds.orch.activity.{e.kind}`), e.texture, nil))
	end
	local ticks_legend: any = { gap = "$space.3", items = "center", shrink = 0 }
	for _, k in LEGEND_TICKS do
		table.insert(ticks_legend, legend_item(locale.t(`ds.orch.map.tick.{k}`), nil, k))
	end

	local function span_button(mins: number): Node
		local function on(): boolean
			return props.window() == mins
		end
		return ui.button {
			class = "orch-tl-window",
			label = locale.t(`ds.orch.map.window_{mins}`),
			selected = on,
			h = 22,
			px = "$space.2",
			rounded = "$radius.sm",
			items = "center",
			bg = function(): string
				return if on() then "$selection.item" else "#00000000"
			end,
			hover = { bg = "$hover" },
			on_click = function()
				props.on_window(mins)
			end,
			ui.text {
				locale.t(`ds.orch.map.span_{mins}`),
				tabular = true,
				text_size = "$font.size.xs",
				weight = "$font.weight.medium",
				color = function(): string
					return if on() then "$text.primary" else "$text.tertiary"
				end,
			},
		}
	end

	return ui.column {
		class = "orch-map-timeline",
		w_full = true,
		gap = "$space.2",
		on_resize = function(size: { w: number, h: number })
			if math.abs(size.w - width:peek()) >= 1 then
				width:set(size.w)
			end
		end,
		ui.row {
			class = "orch-tl-top",
			w_full = true,
			h = TOP_H,
			gap = "$space.4",
			items = "center",
			ui.row(legend),
			ui.show(function(): boolean
				return width:get() >= 820
			end, ui.row(ticks_legend)),
			ui.div { grow = 1 },
			ui.row { gap = 2, items = "center", shrink = 0, span_button(15), span_button(60) },
		},
		ui.column {
			class = "orch-tl-field",
			relative = true,
			w_full = true,
			h = function(): number
				return built:get().h + AXIS_H
			end,
			ui.each(rows, any_row, { key = function(r: Row): string
				return r.id
			end, absolute = true, left = 0, top = 0, right = 0, bottom = 0 }),
			ui.canvas {
				class = "orch-tl-chart",
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				pointer_events = "none",
				draw = draw,
			},
		},
	}
end)

return M

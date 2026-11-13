-- The Map: every machine as a lane, and on it the orchestrators it hosts
-- and the crew threads it runs. An orchestrator is a station in its hue;
-- its crew are pills on its track, a hue line running through them. A
-- thread running on another machine sits on that machine's lane, its
-- orchestrator's line dropping to it there. A pill says how its thread
-- stands (its dot), what it's doing (a glyph: testing, building, merging…)
-- and how far through its todo it is (a thin bar). When agents talk, a
-- pulse runs along the line between them. Click a pill or a station to
-- open it.
--
-- Built for a live view: where things sit is worked out from who is on
-- which machine (and the width), not from what each is doing, so a
-- thread's state changing reaches its pill alone; nothing moves per frame
-- but a pulse, and pulses are animations the renderer plays.
--
--   Map {
--     world = function() return { now, chats, devices, loads, here } end,
--     comms = function() return orchestration.comms:get() end,
--     on_open_chat = function(id) end,
--     on_open_orchestrator = function(id) end,
--   }
--
-- `now` is orchestration.now's; `chats` ds/orch_data.chats; `devices`
-- sessions.now's; `loads` orchestration.loads'; `here` this device's id.

local Icon = require("@self/icon")
local status = require("@self/status")
local locale = require("@self/locale")
local motion = require("@self/motion")
local hue = require("@self/orch_hue")
local data = require("@self/orch_data")

export type Props = {
	world: () -> any,
	comms: () -> { any },
	on_open_chat: (id: string) -> (),
	on_open_orchestrator: (id: string) -> (),
}

-- Geometry (points): the lanes' label column, the station column, a
-- track's height, the pills.
local GUTTER = 0
local PAD = 0
-- Each lane's head: the machine's name and load, over its tracks.
local LANE_HEAD = 30
local ORCH_W = 150
local NODE_H = 28
local TRACK_H = 42
local LANE_PAD = 8
local PILL_H = 30
local PILL_MAX, PILL_MIN = 184, 120
local PILL_GAP = 12
local SPINE_STEP = 8
local CORNER = 8
-- A pulse's run along its line.
local PULSE_MS = 900

local M = {}

type Pill = { id: string, lane: string, x: number, y: number, w: number, orchestrator: string, hue: number? }
type Node_ = { id: string, lane: string, x: number, y: number, hue: number?, paused: boolean }
type Lane = { id: string, name: string, y: number, h: number, online: boolean, here: boolean }
type Edge = { orchestrator: string, hue: number?, paused: boolean, points: { number }, gaps: { { number } } }
export type Layout = {
	h: number,
	lanes: { Lane },
	nodes: { Node_ },
	pills: { Pill },
	more: { { id: string, x: number, y: number, n: number } },
	edges: { Edge },
	-- Where a pulse starts or ends for a chat id: { x, y }.
	anchors: { [string]: { number } },
	-- The line from a crew pill to its station, as points (x, y, …).
	routes: { [string]: { number } },
}

local function device_order(devices: { any }, here: string?): { any }
	local list = table.clone(devices or {})
	table.sort(list, function(a, b)
		local ah, bh = a.id == here or a.here, b.id == here or b.here
		if ah ~= bh then
			return ah == true
		end
		local ac, bc = a.cloud == true or a.platform == "cloud", b.cloud == true or b.platform == "cloud"
		if ac ~= bc then
			return bc
		end
		return (a.name or "") < (b.name or "")
	end)
	return list
end

-- Where everything sits: lanes, stations, pills and the lines between
-- them, for a world and a width. Pure.
function M.layout(world: any, width: number): Layout
	local now = world.now or {}
	local chats = world.chats or {}
	local here = world.here
	local out: Layout = { h = 0, lanes = {}, nodes = {}, pills = {}, more = {}, edges = {}, anchors = {}, routes = {} }

	-- The orchestrators shown, in their order; each one's home and crew.
	local orchs = {}
	for _, o in now.orchestrators or {} do
		if not o.archived then
			table.insert(orchs, o)
		end
	end
	local crew_of: { [string]: { any } } = {}
	for _, m in now.crew or {} do
		if not m.released then
			crew_of[m.orchestrator_id] = crew_of[m.orchestrator_id] or {}
			table.insert(crew_of[m.orchestrator_id], m)
		end
	end
	local items = data.items(now)
	local function home_of(o: any): string?
		local c = chats[o.id]
		return (c and c.device) or here
	end

	-- The lanes: devices online, and any offline one a thread still runs on.
	local used: { [string]: boolean } = {}
	for _, o in orchs do
		local h = home_of(o)
		if h then
			used[h] = true
		end
		for _, m in crew_of[o.id] or {} do
			local c = chats[m.chat_id]
			if c and c.device then
				used[c.device] = true
			end
		end
	end
	local lanes: { any } = {}
	for _, d in device_order(world.devices or {}, here) do
		local online = d.online ~= false or d.id == here or d.here == true
		if online or used[d.id] then
			table.insert(lanes, { id = d.id, name = d.name or "", online = online, here = d.id == here or d.here == true })
		end
	end

	-- Which orchestrators drop a line to another lane: each takes a spine.
	local spine: { [string]: number } = {}
	local spines = 0
	for _, o in orchs do
		local h = home_of(o)
		for _, m in crew_of[o.id] or {} do
			local c = chats[m.chat_id]
			if c and c.device and c.device ~= h and not spine[o.id] then
				spines += 1
				spine[o.id] = spines
			end
		end
	end
	local spine_x0 = GUTTER + PAD + ORCH_W + 12
	local pills_x = spine_x0 + spines * SPINE_STEP + 14
	local room = math.max(PILL_MIN, width - pills_x)

	-- Each orchestrator's track in each lane: { y, pills }.
	local track_y: { [string]: { [string]: number } } = {}
	local y = 0
	for _, lane in lanes do
		local tracks = {}
		for _, o in orchs do
			local home = home_of(o) == lane.id
			local members = {}
			for _, m in crew_of[o.id] or {} do
				local c = chats[m.chat_id]
				local dev = (c and c.device) or home_of(o)
				if dev == lane.id then
					table.insert(members, m)
				end
			end
			if home or #members > 0 then
				table.sort(members, function(a, b)
					local ia, ib = items[a.item_id or ""], items[b.item_id or ""]
					local oa, ob = (ia and ia.order) or 0, (ib and ib.order) or 0
					if oa ~= ob then
						return oa < ob
					end
					return a.chat_id < b.chat_id
				end)
				table.insert(tracks, { o = o, home = home, members = members })
			end
		end
		local h = LANE_HEAD + math.max(1, #tracks) * TRACK_H + LANE_PAD
		table.insert(out.lanes, { id = lane.id, name = lane.name, y = y, h = h, online = lane.online, here = lane.here })
		for ti, t in tracks do
			local ty = y + LANE_HEAD + (ti - 0.5) * TRACK_H
			track_y[t.o.id] = track_y[t.o.id] or {}
			track_y[t.o.id][lane.id] = ty
			local o = t.o
			if t.home then
				table.insert(out.nodes, { id = o.id, lane = lane.id, x = GUTTER + PAD, y = ty - NODE_H / 2, hue = o.hue, paused = o.paused == true })
				out.anchors[o.id] = { GUTTER + PAD + 9, ty }
			end
			-- The pills that fit, evenly wide; the rest a count.
			local n = #t.members
			local w = PILL_MAX
			local shown = n
			if n > 0 then
				w = math.clamp((room - (n - 1) * PILL_GAP) / n, PILL_MIN, PILL_MAX)
				local fit = math.max(1, math.floor((room + PILL_GAP) / (w + PILL_GAP)))
				if fit < n then
					shown = math.max(1, fit - 1)
					w = PILL_MIN
				end
			end
			for i = 1, shown do
				local m = t.members[i]
				local px = pills_x + (i - 1) * (w + PILL_GAP)
				table.insert(out.pills, { id = m.chat_id, lane = lane.id, x = px, y = ty - PILL_H / 2, w = w, orchestrator = o.id, hue = o.hue })
				out.anchors[m.chat_id] = { px + 10, ty }
			end
			if shown < n then
				table.insert(out.more, { id = `{o.id}@{lane.id}`, x = pills_x + shown * (w + PILL_GAP), y = ty - PILL_H / 2, n = n - shown })
			end
		end
		y += h
	end
	out.h = y

	-- The lines: each track from its station (or its spine) through its
	-- pills, gaps left where the pills stand.
	for _, o in orchs do
		local ys = track_y[o.id]
		local home = home_of(o)
		local hy = ys and home and ys[home]
		if ys and hy then
			local start = GUTTER + PAD + ORCH_W
			local sx = if spine[o.id] then spine_x0 + (spine[o.id] - 1) * SPINE_STEP else nil
			for lane_id, ty in ys do
				local pills = {}
				for _, p in out.pills do
					if p.orchestrator == o.id and p.lane == lane_id then
						table.insert(pills, p)
					end
				end
				local last = start
				for _, p in pills do
					last = math.max(last, p.x + p.w)
				end
				for _, mo in out.more do
					if mo.id == `{o.id}@{lane_id}` then
						last = math.max(last, mo.x + 36)
					end
				end
				local gaps = {}
				for _, p in pills do
					table.insert(gaps, { p.x - 3, p.x + p.w + 3 })
				end
				local points
				if lane_id == home then
					if #pills == 0 and not sx then
						continue
					end
					points = { start, ty, math.max(last, sx or start), ty }
				elseif sx then
					points = { start, hy, sx - CORNER, hy, sx, hy, sx, ty, sx + CORNER, ty, last, ty }
				else
					continue
				end
				table.insert(out.edges, { orchestrator = o.id, hue = o.hue, paused = o.paused == true, points = points, gaps = gaps })
				-- Each pill's way back to its station, for a pulse.
				for _, p in pills do
					local px, py = p.x + 10, ty
					if lane_id == home then
						out.routes[p.id] = { px, py, GUTTER + PAD + 9, hy }
					else
						out.routes[p.id] = { px, py, sx :: number, ty, sx :: number, hy, GUTTER + PAD + 9, hy }
					end
				end
			end
		end
	end
	return out
end

-- What the layout is made from, as a string: a layout is worked out again
-- only when it moves.
local function signature(world: any, width: number): string
	local now = world.now or {}
	local chats = world.chats or {}
	local parts = { tostring(math.floor(width)), world.here or "" }
	for _, d in world.devices or {} do
		table.insert(parts, `d{d.id}{d.name}{if d.online == false then 0 else 1}`)
	end
	for _, o in now.orchestrators or {} do
		local c = chats[o.id]
		table.insert(parts, `o{o.id}{o.hue}{if o.paused then 1 else 0}{if o.archived then 1 else 0}{(c and c.device) or ""}`)
	end
	local items = data.items(now)
	for _, m in now.crew or {} do
		if not m.released then
			local c = chats[m.chat_id]
			local it = items[m.item_id or ""]
			table.insert(parts, `c{m.chat_id}{m.orchestrator_id}{(c and c.device) or ""}{(it and it.order) or 0}`)
		end
	end
	return table.concat(parts, "|")
end

local function svg_path(points: { number }): string
	local parts = {}
	for i = 1, #points - 1, 2 do
		table.insert(parts, `{if i == 1 then "M" else "L"}{points[i]} {points[i + 1]}`)
	end
	return table.concat(parts, " ")
end

local function reversed(points: { number }): { number }
	local out = {}
	for i = #points - 1, 1, -2 do
		table.insert(out, points[i])
		table.insert(out, points[i + 1])
	end
	return out
end

M.Map = ui.component(function(props: Props): Node
	local width = ui.use_state(720)
	local last_sig = ""
	local last_layout: Layout? = nil
	local layout = ui.computed(function(): Layout
		local w = width:get()
		local world = props.world()
		local sig = signature(world, w)
		if last_layout and sig == last_sig then
			return last_layout :: Layout
		end
		last_sig = sig
		last_layout = M.layout(world, w)
		return last_layout :: Layout
	end)
	local function chat(id: string): any
		return (props.world().chats or {})[id]
	end
	local function orchestrator(id: string): any
		for _, o in (props.world().now or {}).orchestrators or {} do
			if o.id == id then
				return o
			end
		end
		return nil
	end
	local lanes = ui.computed(function(): { Lane }
		return layout:get().lanes
	end)
	local nodes = ui.computed(function(): { Node_ }
		return layout:get().nodes
	end)
	local pills = ui.computed(function(): { Pill }
		return layout:get().pills
	end)
	local more = ui.computed(function(): { any }
		return layout:get().more
	end)
	local function by_id(x: any): string
		return x.id
	end

	-- The lines, drawn once a layout (or a theme).
	local function draw(c: any, _w: number, _h: number)
		local l = layout:get()
		for _, e in l.edges do
			local ink = hue.canvas(e.hue, if e.paused then 0.28 else (if hue.dark() then 0.55 else 0.62))
			-- The polyline, cut where pills stand (on a horizontal run).
			local pts = e.points
			local path = ui.path()
			local pen = false
			for i = 1, #pts - 3, 2 do
				local x1, y1, x2, y2 = pts[i], pts[i + 1], pts[i + 2], pts[i + 3]
				if y1 == y2 and x2 > x1 then
					-- Horizontal: around the gaps.
					local x = x1
					local cuts = {}
					for _, g in e.gaps do
						if g[2] > x1 and g[1] < x2 and math.abs(y1 - (pts[#pts])) < 0.5 then
							table.insert(cuts, g)
						end
					end
					table.sort(cuts, function(a, b)
						return a[1] < b[1]
					end)
					for _, g in cuts do
						if g[1] > x then
							if not pen then
								path = path:move(x, y1)
							end
							path = path:line(g[1], y1)
						end
						pen = false
						x = math.max(x, g[2])
					end
					if x < x2 then
						if not pen then
							path = path:move(x, y1)
						end
						path = path:line(x2, y2)
						pen = true
					end
				elseif x1 ~= x2 and y1 ~= y2 then
					-- A corner: a quarter round.
					if not pen then
						path = path:move(x1, y1)
					end
					path = path:quad(x2, y1, x2, y2)
					pen = true
				else
					if not pen then
						path = path:move(x1, y1)
					end
					path = path:line(x2, y2)
					pen = true
				end
			end
			c:stroke(path, ink, 1.5, { join = "round", cap = "round" })
		end
		-- A junction where a line leaves for another lane.
		for _, e in l.edges do
			if #e.points > 4 then
				c:circle(e.points[5], e.points[6], 2.5, hue.canvas(e.hue, if e.paused then 0.3 else 0.9))
			end
		end
	end

	-- Lane: the machine's name, and its load.
	local function lane_row(_l: Lane, _i: number, row: State<any>): Node
		local function load(): any
			return (props.world().loads or {})[row:get().id]
		end
		return ui.column {
			class = "orch-lane",
			absolute = true,
			left = 0,
			right = 0,
			top = function(): number
				return row:get().y
			end,
			h = function(): number
				return row:get().h
			end,
			ui.div { absolute = true, left = 0, right = 0, bottom = 0, h = 1, bg = "$border.subtle" },
			ui.row {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				h = LANE_HEAD,
				gap = "$space.2",
				items = "center",
				opacity = function(): number
					return if row:get().online then 1 else 0.5
				end,
				ui.text {
					function(): string
						return row:get().name
					end,
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
					color = "$text.secondary",
					truncate = true,
					min_w = 0,
				},
				ui.show(function(): boolean
					return row:get().here == true
				end, ui.text { locale.t("ds.orch.map.here"), text_size = "$font.size.xs", color = "$text.tertiary", shrink = 0 }),
				ui.canvas {
					class = "orch-lane-load",
					w = 40,
					h = 3,
					shrink = 0,
					label = function(): string
						local l = load()
						return if l and l.cpu then locale.s("ds.orch.map.cpu", { percent = math.floor(l.cpu * 100 + 0.5) }) else ""
					end,
					draw = function(c: any, w: number, h: number)
						local l = load()
						c:rect(0, 0, w, h, "$border", h / 2)
						local f = if l and l.cpu then math.clamp(l.cpu, 0, 1) else 0
						if f > 0 then
							c:rect(0, 0, math.max(h, w * f), h, if f > 0.85 then "$warning" else "$text.tertiary", h / 2)
						end
					end,
				},
				ui.text {
					function(): string
						local l = load()
						if not row:get().online then
							return locale.s("ds.orch.map.offline")
						end
						return if l and l.cpu then `{math.floor(l.cpu * 100 + 0.5)}%` else ""
					end,
					tabular = true,
					shrink = 0,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			},
		}
	end

	-- A station: an orchestrator, in its hue.
	local function node_row(_n: Node_, _i: number, row: State<any>): Node
		local function o(): any
			return orchestrator(row:get().id) or {}
		end
		local function counts(): (number, number)
			local working, total = 0, 0
			for _, m in (props.world().now or {}).crew or {} do
				if m.orchestrator_id == row:get().id and not m.released then
					total += 1
					local c = chat(m.chat_id)
					if c and c.status == "working" then
						working += 1
					end
				end
			end
			return working, total
		end
		return ui.button {
			class = "orch-node",
			label = function(): string
				return o().name or ""
			end,
			absolute = true,
			left = function(): number
				return row:get().x
			end,
			top = function(): number
				return row:get().y
			end,
			w = ORCH_W,
			h = NODE_H,
			pl = "$space.2",
			pr = "$space.2",
			gap = "$space.2",
			items = "center",
			rounded = "$radius.full",
			bg = function(): any
				return hue.wash(row:get().hue)
			end,
			hover = { bg = "$hover" },
			on_click = function()
				props.on_open_orchestrator(row:peek().id)
			end,
			hue.Dot {
				hue = function(): number?
					return row:get().hue
				end,
				size = 9,
				pulse = function(): boolean
					local w = counts()
					return w > 0 and not row:get().paused
				end,
			},
			ui.text {
				function(): string
					return o().name or ""
				end,
				grow = 1,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = function(): string
					return if row:get().paused then "$text.tertiary" else "$text.primary"
				end,
			},
			ui.text {
				function(): string
					if row:get().paused then
						return locale.s("ds.orch.map.paused")
					end
					local w, t = counts()
					return if t > 0 then `{w}/{t}` else ""
				end,
				shrink = 0,
				tabular = true,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
		}
	end

	-- A crew thread: its state, what it's doing, how far along.
	local function pill_row(_p: Pill, _i: number, row: State<any>): Node
		local function c(): any
			return chat(row:get().id) or {}
		end
		local function st(): string
			return c().status or "idle"
		end
		local function activity(): string?
			local a = c().activity
			return if st() == "working" and a then a.kind else nil
		end
		local function frac(): number?
			return data.todo_fraction(c().todo)
		end
		return ui.button {
			class = "orch-pill",
			label = function(): string
				local a = activity()
				local title = c().title or ""
				return if a then `{title}, {locale.s(`ds.orch.activity.{a}`)}` else title
			end,
			tooltip = ui.column {
				gap = "$zeron.space.hair",
				ui.text {
					function(): string
						return c().title or ""
					end,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
				},
				ui.text {
					function(): string
						local a = c().activity
						return if a and st() == "working" then a.label or "" else ""
					end,
					font = "$font.mono",
					text_size = "$font.size.xs",
					color = "$text.secondary",
				},
				ui.text {
					function(): string
						local t = c().todo
						if not t or not t.total or t.total == 0 then
							return ""
						end
						return if t.current then `{t.done}/{t.total} · {t.current}` else `{t.done}/{t.total}`
					end,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			},
			absolute = true,
			left = function(): number
				return row:get().x
			end,
			top = function(): number
				return row:get().y
			end,
			w = function(): number
				return row:get().w
			end,
			h = PILL_H,
			pl = "$space.2",
			pr = "$space.2",
			gap = "$zeron.space.tight",
			items = "center",
			rounded = "$radius.lg",
			border = 1,
			border_color = function(): any
				return if st() == "input" or st() == "failed" then hue.paint(row:get().hue, 0.55) else "$border"
			end,
			bg = "$surface.raised",
			hover = { bg = "$hover" },
			overflow = "hidden",
			on_click = function()
				props.on_open_chat(row:peek().id)
			end,
			ui.switch(function(): string
				local s = st()
				if s == "working" or s == "input" or s == "failed" or s == "done" or s == "approval" or s == "waiting" then
					return s
				end
				return "idle"
			end, {
				working = status.StatusDot { kind = "working", size = 6 },
				waiting = status.StatusDot { kind = "waiting", size = 6 },
				input = status.StatusDot { kind = "input", size = 6 },
				approval = status.StatusDot { kind = "approval", size = 6 },
				failed = status.StatusDot { kind = "failed", size = 6 },
				done = status.StatusDot { kind = "done", size = 6 },
				idle = ui.div { w = 6, h = 6, shrink = 0, rounded = "$radius.full", border = 1, border_color = "$text.tertiary" },
			}),
			ui.show(function(): boolean
				return activity() ~= nil
			end, ui.div {
				color = "$text.secondary",
				shrink = 0,
				Icon {
					name = function(): Icon.Name
						return (data.ACTIVITY :: any)[activity() or ""] or "terminal"
					end,
					size = 12,
				},
			}),
			ui.text {
				function(): string
					return c().title or ""
				end,
				grow = 1,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.sm",
				color = function(): string
					return if st() == "done" or st() == "idle" then "$text.secondary" else "$text.primary"
				end,
			},
			-- How far through its todo.
			ui.show(function(): boolean
				return frac() ~= nil
			end, ui.canvas {
				class = "orch-pill-todo",
				absolute = true,
				left = 10,
				bottom = 3,
				w = function(): number
					return math.max(0, row:get().w - 20)
				end,
				h = 1.5,
				draw = function(cv: any, w: number, h: number)
					local f = frac() or 0
					cv:rect(0, 0, w, h, "$border", h / 2)
					if f > 0 then
						cv:rect(0, 0, math.max(h, w * f), h, hue.canvas(row:get().hue, 0.8), h / 2)
					end
				end,
			}),
		}
	end

	local function more_row(_m: any, _i: number, row: State<any>): Node
		return ui.row {
			absolute = true,
			left = function(): number
				return row:get().x
			end,
			top = function(): number
				return row:get().y
			end,
			h = PILL_H,
			px = "$space.2",
			items = "center",
			ui.text {
				function(): string
					return `+{row:get().n}`
				end,
				tabular = true,
				text_size = "$font.size.sm",
				color = "$text.tertiary",
			},
		}
	end

	-- Pulses: what passes between two agents on the map, run along their
	-- line (or straight across, with none), once each, newest only.
	local pulses = ui.use_state({} :: { any })
	local seen: { [string]: boolean }? = nil
	ui.effect(function()
		local list = props.comms() or {}
		local fresh = {}
		if seen == nil then
			seen = {}
			for _, e in list do
				(seen :: any)[e.id] = true
			end
			return
		end
		for _, e in list do
			if not (seen :: any)[e.id] then
				(seen :: any)[e.id] = true
				table.insert(fresh, e)
			end
		end
		if #fresh == 0 or motion.reduced_now() then
			return
		end
		local l = layout:peek()
		local now_ = props.world().now or {}
		local add = table.clone(pulses:peek())
		for _, e in fresh do
			local a, b = l.anchors[e.from], l.anchors[e.to]
			if a and b then
				local pts
				if l.routes[e.from] then
					pts = l.routes[e.from]
				elseif l.routes[e.to] then
					pts = reversed(l.routes[e.to])
				else
					pts = { a[1], a[2], b[1], b[2] }
				end
				local h = data.hue_of(now_, props.world().chats, e.from) or data.hue_of(now_, props.world().chats, e.to)
				table.insert(add, { id = e.id, path = svg_path(pts), hue = h })
			end
		end
		while #add > 8 do
			table.remove(add, 1)
		end
		pulses:set(add)
	end)
	local function pulse_row(p: any): Node
		local done = false
		return ui.div {
			class = "orch-pulse",
			absolute = true,
			left = -4,
			top = -4,
			w = 8,
			h = 8,
			rounded = "$radius.full",
			pointer_events = "none",
			bg = hue.paint(p.hue),
			shadow = ui.raw(`0 0 8 2 {hue.canvas(p.hue, 0.55)}`, "a pulse's glow, in its hue"),
			on_visible = function(visible: boolean, el: Element)
				if not visible or done then
					return
				end
				done = true
				local mp = anime.svg.createMotionPath(p.path)
				anime.animate(el, {
					x = mp.x,
					y = mp.y,
					opacity = { 0, 1, 1, 0 },
					duration = PULSE_MS,
					ease = "inOutSine",
					on_complete = function()
						local rest = {}
						for _, q in pulses:peek() do
							if q.id ~= p.id then
								table.insert(rest, q)
							end
						end
						pulses:set(rest)
					end,
				})
			end,
		}
	end

	local function empty(): boolean
		return #layout:get().lanes == 0
	end

	return ui.column {
		class = "orch-map",
		w_full = true,
		on_resize = function(size: { w: number, h: number })
			if math.abs(size.w - width:peek()) >= 1 then
				width:set(size.w)
			end
		end,
		ui.show(empty, ui.row {
			w_full = true,
			py = "$space.3",
			ui.text { locale.t("ds.orch.map.empty"), text_size = "$font.size.sm", color = "$text.tertiary" },
		}),
		ui.column {
			class = "orch-map-field",
			relative = true,
			w_full = true,
			h = function(): number
				return layout:get().h
			end,
			ui.each(lanes, lane_row, { key = by_id, absolute = true, left = 0, right = 0, top = 0, bottom = 0 }),
			ui.canvas {
				class = "orch-map-lines",
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				pointer_events = "none",
				draw = draw,
			},
			ui.each(nodes, node_row, { key = by_id, absolute = true, left = 0, right = 0, top = 0, bottom = 0 }),
			ui.each(pills, pill_row, { key = by_id, absolute = true, left = 0, right = 0, top = 0, bottom = 0 }),
			ui.each(more, more_row, { key = by_id, absolute = true, left = 0, right = 0, top = 0, bottom = 0 }),
			ui.each(pulses, pulse_row, { key = by_id, absolute = true, left = 0, right = 0, top = 0, bottom = 0, pointer_events = "none" }),
		},
	}
end)

return M

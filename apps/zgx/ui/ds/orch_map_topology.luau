-- The Map's Topology: each machine a lane, its projects in it, and in
-- them the orchestrators and threads at work, one row each. An
-- orchestrator leads: its rail, in its hue, drops from under its dot, and
-- its crew sit indented under it, an elbow into each, on down across the
-- line between machines for crew on another. When agents talk, a pulse
-- runs along the rail between them. Where everything sits is
-- ds/orch_map_layout's.
--
--   Topology {
--     world = function() return the Map's world end,
--     chat = function(id) return its chat (held 300 ms a state) end,
--     comms = function() return orchestration.comms:get() end,
--     on_open_chat, on_open_orchestrator,
--   }

local Icon = require("@self/icon")
local status = require("@self/status")
local locale = require("@self/locale")
local motion = require("@self/motion")
local hue = require("@self/orch_hue")
local data = require("@self/orch_data")
local kit = require("@self/orch_map_kit")
local Cells = require("@self/orch_cells")
local MachineHead = require("@self/orch_machine_head")
local model = require("@self/orch_map_model")
local Name = require("@ds/orch_name")
local Layout = require("@self/orch_map_layout")
local Visibility = require("@self/orch_map_visibility")

export type Props = {
	world: () -> any,
	chat: (id: string) -> any,
	-- A thread's name by itself (account/rows); else from the world.
	name_of: ((id: string) -> string)?,
	comms: () -> { any },
	on_open_chat: (id: string) -> (),
	on_open_orchestrator: (id: string) -> (),
	-- A secondary click's menu, and a way for the Map to open it (kit.MenuField).
	menu: ((id: string) -> { any }?)?,
	register: ((key: string, open: ((id: string) -> boolean)?) -> ())?,
}

local PULSE_MS = 900

-- How work on other machines is shown: "lines" or "tags" (kept per device).
local TopoStyle = require("@self/orch_map_topo_style")

-- What the layout is made from, as a string: worked out again only when
-- it moves (a thread's state changing reaches its row alone).
local function signature(world: any, width: number): string
	local now = world.now or {}
	local chats = world.chats or {}
	local parts_ = { tostring(math.floor(width)), world.here or "" }
	for _, d in world.devices or {} do
		table.insert(parts_, `d{d.id}{d.name}{if d.online == false then 0 else 1}`)
	end
	for _, p in world.projects or {} do
		table.insert(parts_, `p{p.id}{p.name}`)
	end
	for _, o in now.orchestrators or {} do
		local c = chats[o.id]
		table.insert(parts_, `o{o.id}{o.hue}{if o.paused then 1 else 0}{if o.archived then 1 else 0}{(c and c.device) or ""}{(c and c.project) or ""}`)
	end
	local items = data.items(now)
	for _, m in now.crew or {} do
		if not m.released then
			local c = chats[m.chat_id]
			local it = items[m.item_id or ""]
			table.insert(parts_, `c{m.chat_id}{m.orchestrator_id}{(c and c.device) or ""}{(c and c.project) or ""}{(it and it.order) or 0}`)
		end
	end
	-- (The person's own threads come and go with their state.)
	for id, c in chats do
		if model.shown(c) then
			table.insert(parts_, `t{id}{c.device or ""}{c.project or ""}`)
		end
	end
	table.sort(parts_)
	-- (What the Map's "Show chats" hides or fades now: a cached layout never
	-- keeps a hide.)
	local ctx, clock = model.visibility(world)
	table.insert(parts_, Visibility.key(chats, clock, ctx))
	return table.concat(parts_, "|")
end

local function svg_path(points: { number }): string
	local out = {}
	for i = 1, #points - 1, 2 do
		table.insert(out, `{if i == 1 then "M" else "L"}{points[i]} {points[i + 1]}`)
	end
	return table.concat(out, " ")
end

local function reversed(points: { number }): { number }
	local out = {}
	for i = #points - 1, 1, -2 do
		table.insert(out, points[i])
		table.insert(out, points[i + 1])
	end
	return out
end

-- A polyline as a path: a point between two others on a corner is drawn as
-- a curve through it.
local function polyline(path: any, pts: { number }): any
	local n = #pts // 2
	path = path:move(pts[1], pts[2])
	local i = 2
	while i <= n do
		local x, y = pts[2 * i - 1], pts[2 * i]
		if i < n then
			local px, py = pts[2 * i - 3], pts[2 * i - 2]
			local nx, ny = pts[2 * i + 1], pts[2 * i + 2]
			-- A corner (in vertical, out horizontal or back): curve through it.
			if (px == x) ~= (nx == x) and (py == y) ~= (ny == y) then
				path = path:quad(x, y, nx, ny)
				i += 2
				continue
			end
		end
		path = path:line(x, y)
		i += 1
	end
	return path
end

return ui.component(function(props: Props): Node
	local width = ui.use_state(720)
	local last_sig = ""
	local last_layout: Layout.Layout? = nil
	local layout = ui.computed(function(): Layout.Layout
		local w = width:get()
		local world = props.world()
		local sig = signature(world, w) .. "|" .. TopoStyle.mode()
		if last_layout and sig == last_sig then
			return last_layout :: Layout.Layout
		end
		last_sig = sig
		last_layout = Layout.layout(world, w, TopoStyle.mode())
		return last_layout :: Layout.Layout
	end)
	local lanes = ui.computed(function(): { Layout.Lane }
		return layout:get().lanes
	end)
	local labels = ui.computed(function(): { Layout.Label }
		return layout:get().labels
	end)
	-- (Its rows made a batch a frame as it opens: all of them at once took
	-- a quarter to half a second in one frame on a big fleet. They stand
	-- where the layout puts them, so those still to come move none.)
	local BATCH = 12
	local built = ui.use_state(BATCH)
	local function grow()
		local n = #layout:peek().rows
		if built:peek() < n then
			built:set(built:peek() + BATCH)
			ui.after(0, grow)
		end
	end
	ui.after(0, grow)
	local rows = ui.computed(function(): { Layout.Row }
		local all = layout:get().rows
		local cap = built:get()
		if #all <= cap then
			return all
		end
		return table.move(all, 1, cap, 1, table.create(cap) :: { Layout.Row })
	end)
	-- Each thread's name (ds/orch_name): the crew and items it's looked up in.
	local named = ui.computed(function(): Name.Index
		return Name.index(props.world().now)
	end)
	local given = props.name_of
	local function name_of(id: string): string
		if given then
			return given(id)
		end
		return Name.from(named:get(), props.world().chats, id)
	end
	local function by_id(x: any): string
		return x.id
	end
	local function wide(): boolean
		return width:get() >= 620
	end
	local function roomy(): boolean
		return width:get() >= 460
	end

	-- The rails, drawn once a layout (or a theme): each orchestrator's as
	-- one shape, so where its branches meet its rail nothing darkens.
	local function draw(c: any, _w: number, _h: number)
		local l = layout:get()
		for _, rail in l.rails do
			if rail.edge then
				-- (An edge grouping its rows here: no line from elsewhere.)
				local ink = hue.canvas(rail.hue, if hue.dark() then 0.7 else 0.8)
				local p = kit.path():move(rail.x, rail.top):line(rail.x, rail.bottom)
				c:stroke(p, ink, 2, { cap = "round", dash = if rail.dashed then { 3, 3 } else nil })
				continue
			end
			local alpha = if rail.paused then 0.3 else (if hue.dark() then 0.62 else 0.7)
			local ink = hue.canvas(rail.hue, alpha)
			local path = kit.path()
			for _, pts in Layout.lines(rail) do
				path = polyline(path, pts)
			end
			if rail.dashed then
				-- (What the Brain dump started: lighter, dashed.)
				c:stroke(path, hue.canvas(rail.hue, alpha * 0.8), 1.25, { join = "round", cap = "round", dash = { 3, 4 } })
			else
				c:stroke(path, ink, 1.5, { join = "round", cap = "round" })
			end
		end
	end

	-- A machine's lane: a hairline above it, its head across its top.
	local function lane_row(_l: Layout.Lane, _i: number, row: State<any>): Node
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
			ui.show(function(): boolean
				return row:get().y > 0
			end, ui.div { absolute = true, left = 0, right = 0, top = 0, h = 1, bg = "$border" }),
			ui.column {
				absolute = true,
				left = 0,
				right = 0,
				top = Layout.HEAD_Y,
				opacity = function(): number
					return if row:get().online then 1 else 0.5
				end,
				MachineHead.MachineHead {
					device = function(): any
						return row:get().device
					end,
					here = function(): boolean
						return row:get().here == true
					end,
					load = function(): any
						return ((props.world().loads or {}) :: any)[row:get().id]
					end,
				},
			},
			-- A machine with nothing at work.
			ui.show(function(): boolean
				return (row:get().count or 0) == 0
			end, ui.text {
				locale.t("ds.orch.map.idle"),
				absolute = true,
				left = function(): number
					return row:get().idle_x or (layout:get().content_x + 4)
				end,
				top = Layout.HEAD_H + 6,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			}),
		}
	end

	local function label_row(_l: Layout.Label, _i: number, row: State<any>): Node
		return ui.row {
			class = "orch-project",
			absolute = true,
			left = function(): number
				return row:get().x + 4
			end,
			top = function(): number
				return row:get().y
			end,
			h = Layout.PROJECT_H,
			gap = "$space.1",
			items = "center",
			ui.div { color = "$text.tertiary", shrink = 0, Icon { name = "folder", size = 12 } },
			ui.text {
				function(): string
					return row:get().name
				end,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
		}
	end

	-- An orchestrator's row: its dot, its name, how many of its crew work.
	local function node_row(row: State<any>): Node
		local function me(): any
			for _, x in (props.world().now or {}).orchestrators or {} do
				if x.id == row:get().id then
					return x
				end
			end
			return {}
		end
		-- (Its own state, held 300 ms at least.)
		local ost = kit.held(function(): any
			return props.chat(row:peek().id)
		end)
		local function counts(): (number, number)
			local working, total = 0, 0
			for _, m in (props.world().now or {}).crew or {} do
				local c = props.chat(m.chat_id)
				if m.orchestrator_id == row:get().id and not m.released and not (c and c.archived) then
					total += 1
					if c and c.status == "working" then
						working += 1
					end
				end
			end
			return working, total
		end
		local drag = kit.ref_drag(props.world, function(): string
			return row:peek().id
		end)
		return ui.button {
			class = "orch-node",
			label = function(): string
				return me().name or ""
			end,
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
			h = function(): number
				return row:get().h
			end,
			pl = 4,
			pr = "$space.2",
			gap = "$space.2",
			items = "center",
			rounded = "$radius.md",
			bg = "#00000000",
			hover = { bg = "$hover" },
			on_drag = drag.on_drag,
			drag_preview = drag.drag_preview,
			drag_preview_anchor = drag.drag_preview_anchor,
			drag_preview_offset = drag.drag_preview_offset,
			on_click = function()
				props.on_open_orchestrator(row:peek().id)
			end,
			hue.Dot {
				hue = function(): number?
					return row:get().hue
				end,
				size = 10,
				pulse = function(): boolean
					return ost() == "working" and not row:get().paused
				end,
			},
			ui.text {
				function(): string
					return me().name or ""
				end,
				shrink = 0,
				max_w = function(): number
					return row:get().w * 0.5
				end,
				truncate = true,
				text_size = "$font.size.md",
				weight = "$font.weight.semibold",
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
					return if t > 0 then locale.s("ds.orch.map.crew_working", { working = w, total = t }) else ""
				end,
				grow = 1,
				min_w = 0,
				truncate = true,
				tabular = true,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
			-- What it's doing itself, when it works.
			ui.show(function(): boolean
				return ost() == "working"
			end, ui.div { class = "orch-state", label = status.KINDS.working.long, status.StatusIcon { kind = "working", size = 12 } }),
		}
	end

	-- A thread's row: its state (Zeron's glyph, the working ring), its
	-- title, what it's doing, its todo and for how long.
	local function thread_row(row: State<any>): Node
		local function c(): any
			return props.chat(row:get().id) or {}
		end
		-- (Its state and activity held 300 ms at least.)
		local st, activity = kit.held(c)
		local drag = kit.ref_drag(props.world, function(): string
			return row:peek().id
		end, function(): string?
			return name_of(row:peek().id)
		end)
		return ui.button {
			class = if row:peek().kind == "crew" then "orch-pill" else "orch-thread",
			label = function(): string
				local a = activity()
				local title = name_of(row:get().id)
				return if a then `{title}, {locale.s(`ds.orch.activity.{a.kind}`)}` else title
			end,
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
			h = function(): number
				return row:get().h
			end,
			-- (Fading out: finished a while ago, ds/orch_map_visibility.)
			opacity = function(): number
				return row:get().opacity or 1
			end,
			pl = 4,
			pr = "$space.2",
			gap = "$space.2",
			items = "center",
			rounded = "$radius.md",
			bg = "#00000000",
			hover = { bg = "$hover" },
			on_drag = drag.on_drag,
			drag_preview = drag.drag_preview,
			drag_preview_anchor = drag.drag_preview_anchor,
			drag_preview_offset = drag.drag_preview_offset,
			on_click = function()
				props.on_open_chat(row:peek().id)
			end,
			ui.div {
				w = 14,
				h = 14,
				shrink = 0,
				items = "center",
				justify = "center",
				class = "orch-state",
				-- (Its state in words, as the sidebar's dot says it.)
				label = function(): string
					local k = st()
					return if k == "idle" then "" else status.KINDS[k :: any].long
				end,
				ui.switch(function(): string
					local s = st()
					return if s == "idle" then "idle" else "state"
				end, {
					idle = ui.div { w = 7, h = 7, rounded = "$radius.full", border = 1, border_color = "$text.tertiary" },
					state = status.StatusIcon {
						kind = function(): any
							return st()
						end,
						size = 14,
					},
				}),
			},
			ui.text {
				function(): string
					return name_of(row:get().id)
				end,
				shrink = 0,
				max_w = function(): number
					return row:get().w * (if wide() then 0.36 else 0.5)
				end,
				truncate = true,
				text_size = "$font.size.sm",
				color = function(): string
					local s = st()
					return if s == "done" or s == "idle" then "$text.secondary" else "$text.primary"
				end,
			},
			-- ("tags": a row away from its orchestrator's machine names
			-- where it came from, in that orchestrator's hue.)
			ui.show(function(): boolean
				return row:get().origin ~= nil
			end, ui.row {
				class = "orch-origin",
				gap = 4,
				shrink = 0,
				items = "center",
				hue.Dot { hue = function(): number?
					return row:get().hue
				end, size = 6 },
				ui.text {
					function(): string
						local id = row:get().origin
						for _, o in (props.world().now or {}).orchestrators or {} do
							if o.id == id then
								return `↳ {o.name or ""}`
							end
						end
						return ""
					end,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			}),
			-- What it's doing: its word, then what it's running.
			ui.text {
				function(): string
					local a = activity()
					local s = st()
					if a then
						return locale.s(`ds.orch.activity.{a.kind}`)
					elseif s == "input" or s == "approval" or s == "failed" or s == "done" then
						return status.KINDS[s :: any].long
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
					return "$text.secondary"
				end,
			},
			ui.text {
				function(): string
					local a = activity()
					return if a and wide() then a.label or "" else ""
				end,
				literal = true,
				grow = 1,
				min_w = 0,
				truncate = true,
				font = "$font.mono",
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
			ui.show(function(): boolean
				local t = c().todo
				return roomy() and t ~= nil and (t.total or 0) > 0
			end, Cells {
				done = function(): number
					return (c().todo or {}).done or 0
				end,
				total = function(): number
					return (c().todo or {}).total or 0
				end,
				failed = function(): number
					return if st() == "failed" then 1 else 0
				end,
				current = function(): boolean
					return st() == "working"
				end,
				cell = 3,
				h = 10,
			}),
			ui.text {
				function(): string
					local a = activity()
					if a and a.since then
						return status.elapsed(kit.since(a.since))
					end
					local u = c().updated
					return if u then status.ago(kit.since(u)) else ""
				end,
				shrink = 0,
				w = 34,
				align = "end",
				tabular = true,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
		}
	end

	-- (A row's kind is its id's for good: an orchestrator stays one.)
	local function any_row(r: Layout.Row, _i: number, row: State<any>): Node
		return if r.kind == "orchestrator" then node_row(row) else thread_row(row)
	end

	-- Pulses: what passes between two agents on the map, run along their
	-- way, once each, the newest few.
	local pulses = ui.use_state({} :: { any })
	local seen: { [string]: boolean }? = nil
	ui.effect(function()
		local list = props.comms() or {}
		if seen == nil then
			seen = {}
			for _, e in list do
				(seen :: any)[e.id] = true
			end
			return
		end
		local fresh = {}
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
		local world = props.world()
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
				local h = data.hue_of(world.now, world.chats, e.from) or data.hue_of(world.now, world.chats, e.to)
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
			left = -6,
			top = -6,
			w = 12,
			h = 12,
			items = "center",
			justify = "center",
			rounded = "$radius.full",
			pointer_events = "none",
			bg = hue.paint(p.hue, 0.3),
			ui.div { w = 5, h = 5, rounded = "$radius.full", bg = hue.paint(p.hue) },
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

	return ui.column {
		class = "orch-map-topology",
		w_full = true,
		on_resize = function(size: { w: number, h: number })
			if math.abs(size.w - width:peek()) >= 1 then
				width:set(size.w)
			end
		end,
		ui.show(function(): boolean
			return #layout:get().lanes == 0
		end, ui.row {
			w_full = true,
			py = "$space.3",
			ui.text { locale.t("ds.orch.map.empty"), text_size = "$font.size.sm", color = "$text.tertiary" },
		}),
		-- A secondary click on a row (a machine's head): its menu.
		kit.MenuField {
			menu = props.menu,
			register = props.register,
			key = "topology",
			hit = function(x: number, y: number): string?
				local l = layout:peek()
				for _, r in l.rows do
					if x >= r.x and x <= r.x + r.w and y >= r.y and y < r.y + r.h then
						return r.id
					end
				end
				for _, lane in l.lanes do
					if y >= lane.y and y < lane.y + Layout.HEAD_H then
						return `dev:{lane.id}`
					end
				end
				return nil
			end,
			center = function(id: string): (number?, number?)
				local l = layout:peek()
				for _, r in l.rows do
					if r.id == id then
						return r.x + 24, r.mid
					end
				end
				for _, lane in l.lanes do
					if `dev:{lane.id}` == id then
						return 24, lane.y + Layout.HEAD_Y + 14
					end
				end
				return nil, nil
			end,
			child = ui.column {
			class = "orch-map-field",
			id = "orch-map-field",
			relative = true,
			w_full = true,
			h = function(): number
				return layout:get().h
			end,
			ui.each(lanes, lane_row, { key = by_id, absolute = true, left = 0, right = 0, top = 0, bottom = 0 }),
			ui.each(labels, label_row, { key = by_id, absolute = true, left = 0, top = 0, w = 0, h = 0 }),
			-- (In bands: one canvas the field's height would pass zgx's
			-- raster limit on a big fleet and lose the window its frame.)
			kit.Tiled {
				class = "orch-map-lines",
				h = function(): number
					return layout:get().h
				end,
				draw = draw,
			},
			ui.each(rows, any_row, { key = by_id, absolute = true, left = 0, top = 0, w = 0, h = 0 }),
			ui.each(pulses, pulse_row, { key = by_id, absolute = true, left = 0, top = 0, w = 0, h = 0, pointer_events = "none" }),
		},
		},
	}
end)

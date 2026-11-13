-- The Map's Graph: everything at work as a web. Machines are the large
-- quiet nodes (their mark, their CPU as a ring), orchestrators filled in
-- their hue, the Brain dump a diamond in its own, threads small dots in
-- their state (working bright and turning, waiting on you in input's ink,
-- failed in danger's, finished faded and smaller; the person's own threads
-- open rings). Lines say what runs where (faint, short), what leads what
-- (an orchestrator to its crew, in its hue), what the Brain dump started
-- (dashed) or handed a part to (dotted, thinner); a pulse runs along a line
-- as agents talk. It settles as a web (ds/orch_map_sim) and is still once
-- settled: no work while nothing changes.
--
-- Drag a node to place it (it stays, until Reset); drag the space or
-- scroll to pan, pinch or ⌘/Ctrl-scroll to zoom. The pointer on a node
-- lights it and what it touches, dims the rest and shows its card; a
-- click opens it.
--
--   Graph { world, chat, comms, on_open_chat, on_open_orchestrator }

local Icon = require("@self/icon")
local IconButton = require("@self/icon_button").IconButton
local HoverCard = require("@self/hover_card")
local status = require("@self/status")
local locale = require("@self/locale")
local motion = require("@self/motion")
local hue = require("@self/orch_hue")
local kit = require("@self/orch_map_kit")
local model = require("@self/orch_map_model")
local sim = require("@self/orch_map_sim")
local Cells = require("@self/orch_cells")
local ContextUsage = require("@self/context_usage")
local MachineHead = require("@self/orch_machine_head")

local M = {}

local PULSE_MS = 900
-- Where nodes were dropped, on this device: { [id] = { x, y } }.
M.pins = ui.persist("control_map_graph_pins", {} :: { [string]: { number } })
-- The node the pointer is on (its id), for what lights up.
M.hover = ui.state("control_map_graph_hover", nil :: string?)

local function sig_of(g: sim.Graph): string
	local parts = {}
	for _, n in g.nodes do
		table.insert(parts, `{n.kind}:{n.id}`)
	end
	for _, e in g.edges do
		table.insert(parts, `{e.kind}:{e.a}>{e.b}`)
	end
	table.sort(parts)
	return table.concat(parts, "|")
end

-- A machine's mark, drawn: a screen, a laptop, a cloud.
local function glyph(c: any, kind: string, x: number, y: number, ink: string)
	if kind == "cloud" then
		local p = ui.path():arc(x - 3.5, y + 1, 3.5, 90, 180):arc(x, y - 1.5, 4.5, 180, 180):arc(x + 4, y + 1, 3.5, 270, 180):close()
		c:stroke(p, ink, 1.25, { join = "round" })
	elseif kind == "laptop" then
		c:stroke(ui.path():rect(x - 5.5, y - 4.5, 11, 7.5, 1.5), ink, 1.25)
		c:line(x - 7.5, y + 4.5, x + 7.5, y + 4.5, ink, 1.25, { cap = "round" })
	else
		c:stroke(ui.path():rect(x - 6.5, y - 5.5, 13, 8.5, 1.5), ink, 1.25)
		c:line(x, y + 3, x, y + 5.5, ink, 1.25)
		c:line(x - 3, y + 5.5, x + 3, y + 5.5, ink, 1.25, { cap = "round" })
	end
end

M.View = ui.component(function(props: {
	world: () -> any,
	chat: (id: string) -> any,
	comms: () -> { any },
	on_open_chat: (id: string) -> (),
	on_open_orchestrator: (id: string) -> (),
}): Node
	local width = ui.use_state(720)
	local function height(): number
		return math.clamp(math.floor(width:get() * 0.6), 380, 660)
	end
	-- The web as it stands (positions move in place; `tick` says redraw),
	-- which nodes it has (`version`), where the view is.
	local graph: sim.Graph = { nodes = {}, edges = {}, index = {}, alpha = 0 }
	local last_sig = ""
	local tick = ui.use_state(0)
	local version = ui.use_state(0)
	local settled = ui.use_state(false)
	local view = ui.use_state({ x = 360, y = 220, zoom = 1 })
	-- Whether the person moved the view themselves: until then it fits
	-- the web as it settles and as the Map's width changes.
	local moved_view = false
	local pulses: { any } = {}
	-- Each node's state as shown (held 300 ms at least), by id: its marker
	-- keeps it, the drawing reads it.
	local shown: { [string]: () -> string } = {}

	local function fit()
		local x0, y0, x1, y1 = sim.bounds(graph)
		local w, h = width:peek(), math.clamp(math.floor(width:peek() * 0.6), 380, 660)
		-- (A small web closer, a big one as close as names stay readable.)
		local most = if #graph.nodes <= 12 then 1.6 else 1.15
		local zoom = math.clamp(math.min((w - 120) / math.max(1, x1 - x0), (h - 110) / math.max(1, y1 - y0)), 0.35, most)
		view:set({ x = w / 2 - (x0 + x1) / 2 * zoom, y = h / 2 - (y0 + y1) / 2 * zoom + 10, zoom = zoom })
	end

	-- The settling, a frame at a time, until still; then once more for
	-- what's laid over it (markers, rings). A new web starts a new run.
	local run = 0
	local function pump()
		run += 1
		local mine = run
		settled:set(false)
		local function frame()
			if mine ~= run then
				return
			end
			local moving = sim.step(graph)
			moving = sim.step(graph) or moving
			local now = ui.now()
			local live = {}
			for _, p in pulses do
				if now - p.t0 < PULSE_MS then
					table.insert(live, p)
				end
			end
			pulses = live
			if not moving and not moved_view then
				fit()
			end
			tick:set(tick:peek() + 1)
			if moving or #pulses > 0 then
				ui.after(16, frame)
			else
				settled:set(true)
			end
		end
		ui.after(0, frame)
	end

	-- A new web when what's in it changes (not when a state does): its
	-- nodes where they were, dropped ones where dropped.
	ui.effect(function()
		local g = sim.build(props.world())
		local sig = sig_of(g)
		if sig == last_sig then
			return
		end
		last_sig = sig
		sim.carry(g, if #graph.nodes > 0 then graph else nil)
		for id, at in M.pins:peek() do
			local i = g.index[id]
			if i then
				local n = g.nodes[i]
				n.x, n.y, n.fixed = at[1], at[2], true
			end
		end
		local fresh = #graph.nodes == 0
		g.alpha = if fresh then 1 else 0.35
		graph = g
		version:set(version:peek() + 1)
		if motion.reduced_now() then
			sim.settle(graph, 600)
			fit()
			tick:set(tick:peek() + 1)
			settled:set(true)
			return
		end
		-- (Most of the settling at once, the last of it seen.)
		if fresh then
			sim.settle(graph, 160)
			fit()
		end
		pump()
	end)

	-- Pulses: what passes between two agents, along their line, once each.
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
		local fresh = false
		for _, e in list do
			if not (seen :: any)[e.id] then
				(seen :: any)[e.id] = true
				if graph.index[e.from] and graph.index[e.to] and not motion.reduced_now() then
					local world = props.world()
					local o = (world.now or {}).orchestrators or {}
					local h = nil
					for _, x in o do
						if x.id == e.from or x.id == e.to then
							h = x.hue
						end
					end
					table.insert(pulses, { from = e.from, to = e.to, t0 = ui.now(), hue = h })
					fresh = true
				end
			end
		end
		if fresh and settled:peek() then
			pump()
		end
	end)

	local function screen(n: sim.Node, v: any): (number, number)
		return n.x * v.zoom + v.x, n.y * v.zoom + v.y
	end
	local function hit(px: number, py: number): sim.Node?
		local v = view:peek()
		local best, best_d = nil, math.huge
		for _, n in graph.nodes do
			local sx, sy = screen(n, v)
			local d = math.sqrt((sx - px) ^ 2 + (sy - py) ^ 2)
			local room = math.max(n.r * v.zoom + 4, 9)
			if d <= room and d < best_d then
				best, best_d = n, d
			end
		end
		return best
	end
	local function state_of(id: string): string
		local f = shown[id]
		if f then
			return f()
		end
		return model.status_of(props.chat(id))
	end

	local function draw(c: any, _w: number, _h: number)
		tick:get()
		local v = view:get()
		local over = M.hover:get()
		local near = if over and graph.index[over] then sim.neighbors(graph, over) else nil
		local z = v.zoom
		local function dim(a: number, lit: boolean): number
			return if near and not lit then a * 0.18 else a
		end
		-- Lines first.
		for _, e in graph.edges do
			local a, b = graph.nodes[graph.index[e.a]], graph.nodes[graph.index[e.b]]
			local ax, ay = screen(a, v)
			local bx, by = screen(b, v)
			local lit = near ~= nil and (e.a == over or e.b == over)
			if e.kind == "host" then
				c:line(ax, ay, bx, by, kit.ink("text.primary", dim(if lit then 0.22 else 0.09, lit)), 1)
			elseif e.kind == "control" then
				c:line(ax, ay, bx, by, hue.canvas(e.hue, dim(if lit then 0.9 else 0.55, lit)), if lit then 1.75 else 1.35, { cap = "round" })
			elseif e.kind == "started" then
				c:line(ax, ay, bx, by, hue.canvas(e.hue, dim(if lit then 0.85 else 0.5, lit)), 1.25, { cap = "round", dash = { 4, 4 } })
			else
				c:line(ax, ay, bx, by, hue.canvas(e.hue, dim(if lit then 0.6 else 0.26, lit)), 1, { cap = "round", dash = { 1, 4 } })
			end
		end
		-- Pulses along them.
		local now = ui.now()
		for _, p in pulses do
			local a, b = graph.nodes[graph.index[p.from] or 0], graph.nodes[graph.index[p.to] or 0]
			if a and b then
				local t = math.clamp((now - p.t0) / PULSE_MS, 0, 1)
				t = t * t * (3 - 2 * t)
				local ax, ay = screen(a, v)
				local bx, by = screen(b, v)
				local x, y = ax + (bx - ax) * t, ay + (by - ay) * t
				c:circle(x, y, 6, hue.canvas(p.hue, 0.25))
				c:circle(x, y, 2.75, hue.canvas(p.hue, 1))
			end
		end
		-- Then the nodes, machines under the rest.
		local loads = props.world().loads or {}
		local here = props.world().here
		for pass = 1, 2 do
			for _, n in graph.nodes do
				if (n.kind == "device") ~= (pass == 1) then
					continue
				end
				local lit = near == nil or near[n.id] == true
				local x, y = screen(n, v)
				local r = n.r * z
				if n.kind == "device" then
					local fill = if (graph.nodes[graph.index[n.id]] :: any) then kit.ink("text.primary", dim(0.06, lit)) else ""
					c:circle(x, y, r, kit.ink("background", dim(0.92, lit)))
					c:circle(x, y, r, fill)
					c:stroke(ui.path():arc(x, y, r, 0, 360), kit.ink("text.primary", dim(0.2, lit)), 1)
					local l = (loads :: any)[n.device or ""]
					local f = if l and l.cpu then math.clamp(l.cpu, 0, 1) else 0
					c:stroke(ui.path():arc(x, y, r + 4, 0, 360), kit.ink("text.primary", dim(0.08, lit)), 2)
					if f > 0 then
						local ink = ContextUsage.ink(f)
						c:stroke(ui.path():arc(x, y, r + 4, -90, 360 * f), kit.ink(string.sub(ink, 2), dim(1, lit)), 2, { cap = "round" })
					end
					local d = nil
					for _, dev in props.world().devices or {} do
						if dev.id == n.device then
							d = dev
						end
					end
					glyph(c, MachineHead.icon(d), x, y, kit.ink("text.primary", dim(0.62, lit)))
					if n.device == here or (d and d.here) then
						c:circle(x + r * 0.72, y - r * 0.72, 4.5, kit.ink("zeron.settings.card", 1))
						c:circle(x + r * 0.72, y - r * 0.72, 3, kit.ink("zeron.tone.info.ink", dim(1, lit)))
					end
				elseif n.kind == "orchestrator" then
					c:circle(x, y, r + 4 * z, hue.canvas(n.hue, dim(0.16, lit)))
					c:circle(x, y, r, hue.canvas(n.hue, dim(1, lit)))
				elseif n.kind == "brain" then
					c:circle(x, y, r + 4 * z, hue.canvas(n.hue, dim(0.16, lit)))
					local p = ui.path():move(x, y - r * 1.15):line(x + r * 1.15, y):line(x, y + r * 1.15):line(x - r * 1.15, y):close()
					c:fill(p, hue.canvas(n.hue, dim(1, lit)))
				else
					local st = state_of(n.id)
					local faded = st == "done" or st == "idle"
					local rr = if faded then r * 0.75 else r
					local ink
					if st == "working" or st == "waiting" then
						-- (Small inside Zeron's working ring, laid over it.)
						ink = kit.ink("text.primary", dim(0.85, lit))
						rr = r * 0.62
					elseif st == "input" or st == "approval" or st == "failed" then
						ink = kit.ink(`zeron.status.{st}`, dim(1, lit))
						c:circle(x, y, rr + 4 * z, kit.ink(`zeron.status.{st}`, dim(0.22, lit)))
					elseif st == "done" then
						ink = kit.ink("zeron.status.done", dim(0.55, lit))
					else
						ink = kit.ink("text.primary", dim(0.3, lit))
					end
					if n.crew then
						c:circle(x, y, rr, ink)
					else
						c:circle(x, y, rr, kit.ink("background", dim(0.9, lit)))
						c:stroke(ui.path():arc(x, y, rr - 0.75, 0, 360), ink, 1.5)
					end
				end
			end
		end
		-- Names: machines, orchestrators and the Brain dump always; threads
		-- close up, or lit.
		for _, n in graph.nodes do
			local lit = near == nil or near[n.id] == true
			local x, y = screen(n, v)
			local r = n.r * z
			if n.kind == "device" then
				c:text(n.title, x, y + r + 9, { size = 12, weight = 500, color = kit.ink("text.primary", dim(0.9, lit)), align = "center", baseline = "top" })
			elseif n.kind == "orchestrator" or n.kind == "brain" then
				c:text(n.title, x + r + 7, y, { size = 12, weight = 600, color = kit.ink("text.primary", dim(0.95, lit)), baseline = "middle" })
			elseif z >= 1.6 or (near ~= nil and near[n.id]) then
				c:text(n.title, x + r + 6, y, { size = 11, color = kit.ink("text.primary", dim(0.7, lit)), baseline = "middle" })
			end
		end
	end

	-- The pointer: a press on a node may become a drag (the node follows,
	-- placed where let go) or a click (it opens); on the space, a pan.
	local press: any = nil
	local function to_world(px: number, py: number): (number, number)
		local v = view:peek()
		return (px - v.x) / v.zoom, (py - v.y) / v.zoom
	end
	local function open(n: sim.Node)
		if n.kind == "thread" then
			props.on_open_chat(n.id)
		elseif n.kind == "orchestrator" or n.kind == "brain" then
			props.on_open_orchestrator(n.id)
		end
	end
	local function zoom_at(px: number, py: number, k: number)
		local v = view:peek()
		local zoom = math.clamp(v.zoom * k, 0.3, 3)
		local wx, wy = (px - v.x) / v.zoom, (py - v.y) / v.zoom
		view:set({ x = px - wx * zoom, y = py - wy * zoom, zoom = zoom })
	end

	-- A node's marker: where it is, for assistive technology and tests,
	-- and Zeron's working ring round a thread at work (once settled).
	local nodes = ui.computed(function(): { { id: string, kind: string, title: string } }
		version:get()
		local out = {}
		for _, n in graph.nodes do
			table.insert(out, { id = n.id, kind = n.kind, title = n.title })
		end
		return out
	end)
	local function marker(item: { id: string, kind: string, title: string }): Node
		local st = if item.kind == "thread" or item.kind == "orchestrator" or item.kind == "brain"
			then kit.held(function(): any
				return props.chat(item.id)
			end)
			else nil
		if st then
			shown[item.id] = st
		end
		local function node(): sim.Node?
			version:get()
			return graph.nodes[graph.index[item.id] or 0]
		end
		local function at(): (number, number, number)
			settled:get()
			tick:get()
			local n = node()
			if not n then
				return -1000, -1000, 0
			end
			local v = view:get()
			local x, y = screen(n, v)
			return x, y, math.max(n.r * v.zoom, 5)
		end
		return ui.div {
			class = `orch-graph-node orch-graph-{item.kind}`,
			label = function(): string
				local s = if st then st() else "idle"
				if s ~= "idle" and status.KINDS[s :: any] then
					return `{item.title}, {status.KINDS[s :: any].long}`
				end
				return item.title
			end,
			absolute = true,
			pointer_events = "none",
			left = function(): number
				local x, _, r = at()
				return x - r
			end,
			top = function(): number
				local _, y, r = at()
				return y - r
			end,
			w = function(): number
				local _, _, r = at()
				return r * 2
			end,
			h = function(): number
				local _, _, r = at()
				return r * 2
			end,
			items = "center",
			justify = "center",
			ui.show(function(): boolean
				return item.kind == "thread" and st ~= nil and (st() == "working" or st() == "waiting") and settled:get()
			end, ui.div {
				shrink = 0,
				opacity = function(): number
					local over = M.hover:get()
					if over and graph.index[over] and not sim.neighbors(graph, over)[item.id] then
						return 0.2
					end
					return 1
				end,
				status.StatusIcon { kind = "working", size = 22 },
			}),
		}
	end

	-- The card of the node the pointer is on.
	local function card(): Node
		local function n(): sim.Node?
			local id = M.hover:get()
			version:get()
			return if id then graph.nodes[graph.index[id] or 0] else nil
		end
		local function c(): any
			local x = n()
			return if x then props.chat(x.id) or {} else {}
		end
		local function st(): string
			local x = n()
			return if x then state_of(x.id) else "idle"
		end
		local function device_name(): string
			local x = n()
			for _, d in props.world().devices or {} do
				if x and d.id == x.device then
					return d.name or ""
				end
			end
			return ""
		end
		return HoverCard.Card {
			class = "orch-graph-card",
			absolute = true,
			pointer_events = "none",
			left = function(): number
				local x = n()
				if not x then
					return -1000
				end
				local v = view:get()
				tick:get()
				local sx = x.x * v.zoom + v.x
				-- (Under the node, its name left clear.)
				return math.clamp(sx - 124, 8, width:get() - 256)
			end,
			top = function(): number
				local x = n()
				if not x then
					return -1000
				end
				local v = view:get()
				tick:get()
				local sy = x.y * v.zoom + v.y
				local below = sy + x.r * v.zoom + 14
				-- (Near the bottom: above it instead.)
				return if below > height() - 130 then math.max(8, sy - x.r * v.zoom - 140) else below
			end,
			w = 248,
			p = "$space.3",
			gap = "$space.1",
			ui.row {
				gap = "$space.2",
				items = "center",
				ui.show(function(): boolean
					local x = n()
					return x ~= nil and x.kind == "thread" and st() ~= "idle"
				end, status.StatusIcon { kind = function(): any
					return st()
				end, size = 14 }),
				ui.text {
					function(): string
						local x = n()
						return if x then x.title else ""
					end,
					grow = 1,
					min_w = 0,
					truncate = true,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					color = "$text.primary",
				},
			},
			ui.text {
				function(): string
					local x = n()
					if not x then
						return ""
					end
					if x.kind == "device" then
						local l = ((props.world().loads or {}) :: any)[x.device or ""]
						return if l and l.cpu then locale.s("ds.orch.map.cpu", { percent = math.floor(l.cpu * 100 + 0.5) }) else ""
					end
					local s = st()
					local a = c().activity
					if s == "working" and a then
						return locale.s(`ds.orch.activity.{a.kind}`)
					end
					return if s ~= "idle" and status.KINDS[s :: any] then status.KINDS[s :: any].long else ""
				end,
				text_size = "$font.size.xs",
				color = "$text.secondary",
			},
			ui.text {
				function(): string
					local a = c().activity
					return if st() == "working" and a then a.label or "" else ""
				end,
				literal = true,
				truncate = true,
				w_full = true,
				font = "$font.mono",
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				ui.div { color = "$text.tertiary", shrink = 0, Icon { name = "monitor", size = 12 } },
				ui.text { device_name, text_size = "$font.size.xs", color = "$text.tertiary", truncate = true, min_w = 0 },
				ui.text {
					function(): string
						return c().model or ""
					end,
					grow = 1,
					align = "end",
					truncate = true,
					min_w = 0,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			},
			ui.show(function(): boolean
				local t = c().todo
				return t ~= nil and (t.total or 0) > 0
			end, Cells {
				done = function(): number
					return (c().todo or {}).done or 0
				end,
				total = function(): number
					return (c().todo or {}).total or 0
				end,
				current = function(): boolean
					return st() == "working"
				end,
				fill = true,
				w = 224,
				h = 8,
			}),
		}
	end

	return ui.column {
		class = "orch-map-graph",
		w_full = true,
		on_resize = function(size: { w: number, h: number })
			if math.abs(size.w - width:peek()) >= 1 then
				width:set(size.w)
				if not moved_view then
					fit()
				end
			end
		end,
		ui.div {
			class = "orch-graph-field",
			id = "orch-graph-field",
			relative = true,
			w_full = true,
			h = height,
			overflow = "hidden",
			rounded = "$radius.lg",
			on_press = function(e: any)
				local n = hit(e.local_x, e.local_y)
				press = { id = n and n.id, x = e.local_x, y = e.local_y, moved = false, view = view:peek() }
			end,
			on_pointer_move = function(e: any)
				if press then
					local dx, dy = e.local_x - press.x, e.local_y - press.y
					if not press.moved and dx * dx + dy * dy < 16 then
						return
					end
					press.moved = true
					if press.id then
						local n = graph.nodes[graph.index[press.id] or 0]
						if n then
							n.x, n.y = to_world(e.local_x, e.local_y)
							n.fixed = true
							graph.alpha = math.max(graph.alpha, 0.12)
							if settled:peek() then
								pump()
							end
						end
					else
						moved_view = true
						view:set({ x = press.view.x + dx, y = press.view.y + dy, zoom = press.view.zoom })
					end
					return
				end
				local n = hit(e.local_x, e.local_y)
				local id = n and n.id
				if id ~= M.hover:peek() then
					M.hover:set(id)
				end
			end,
			on_release = function(e: any)
				local p = press
				press = nil
				if not p then
					return
				end
				if not p.moved then
					local n = hit(e.local_x, e.local_y)
					if n then
						open(n)
					end
				elseif p.id then
					local n = graph.nodes[graph.index[p.id] or 0]
					if n then
						M.pins:set_key({ p.id }, { n.x, n.y })
					end
				end
			end,
			on_hover = function(inside: boolean)
				if not inside and not press then
					M.hover:set(nil)
				end
			end,
			on_wheel = function(e: any): boolean
				moved_view = true
				if e.ctrl or e.cmd then
					zoom_at(e.local_x, e.local_y, math.exp(-e.dy * 0.01))
				else
					local v = view:peek()
					view:set({ x = v.x - e.dx, y = v.y - e.dy, zoom = v.zoom })
				end
				return true
			end,
			on_pinch = function(e: any): boolean
				moved_view = true
				zoom_at(e.local_x, e.local_y, e.scale)
				return true
			end,
			-- (Drawn in the field's own points: what keeps its size on screen,
			-- the lines' widths, the names, would grow under a view's zoom.)
			-- zgx-lint: allow(canvas-view)
			ui.canvas {
				class = "orch-graph-canvas",
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				pointer_events = "none",
				label = function(): string
					return locale.s("ds.orch.map.variant.graph")
				end,
				draw = draw,
			},
			ui.each(nodes, function(item: any): Node
				return marker(item)
			end, { key = function(item: any): string
				return item.id
			end, absolute = true, left = 0, top = 0, right = 0, bottom = 0, pointer_events = "none" }),
			ui.show(function(): boolean
				return M.hover:get() ~= nil
			end, card()),
			-- Fit it all in; put every node back where the web would.
			ui.row {
				absolute = true,
				right = 8,
				top = 8,
				gap = "$space.1",
				IconButton {
					id = "orch-graph-fit",
					icon = "expand-arrows",
					label = locale.t("ds.orch.map.fit"),
					size = "sm",
					tone = "quiet",
					on_click = function()
						moved_view = false
						fit()
					end,
				},
				IconButton {
					id = "orch-graph-reset",
					icon = "refresh",
					label = locale.t("ds.orch.map.reset"),
					size = "sm",
					tone = "quiet",
					on_click = function()
						M.pins:set({})
						for _, n in graph.nodes do
							n.fixed = false
						end
						graph.alpha = 0.8
						moved_view = false
						if motion.reduced_now() then
							sim.settle(graph, 600)
							fit()
							tick:set(tick:peek() + 1)
						else
							pump()
						end
					end,
				},
			},
		},
	}
end)

return M

-- The Map's Graph: everything at work as a web. Each machine is an island:
-- a quiet region round all it runs, its name set in the rim and its CPU
-- running along it, the machine a square plate at its middle (hardware
-- square, agents round). Orchestrators are filled in their hue inside a
-- ring of it, the Brain dump a diamond in its own, threads small dots in
-- their state (working bright and turning, waiting on you in input's ink,
-- failed in danger's, finished faded and smaller; the person's own threads
-- open rings). The island says what runs where (a dashed line home only
-- for what strays off it); curves say what leads what (an orchestrator to
-- its crew, in its hue), what the Brain dump started
-- (dashed) or handed a part to (dotted, thinner); a pulse runs along a line
-- as agents talk. It settles as a web (ds/orch_map_sim) and is still once
-- settled: no work while nothing changes.
--
-- Drag a node to place it (it stays, until Reset); drag the space to
-- pan; pinch or ⌘/Ctrl-scroll to zoom (a plain scroll is the page's).
-- The pointer on a node lights it and what it touches, dims the rest
-- and shows its card; a click opens it.
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
		local most = if #graph.nodes <= 12 then 1.25 else 1.1
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

	-- An island's room past its farthest member (its name and ring), and
	-- the most it spreads (a member pulled farther keeps a line home).
	local ISLAND_PAD, ISLAND_MAX = 26, 170

	local function draw(c: any, w: number, h: number)
		tick:get()
		local v = view:get()
		local over = M.hover:get()
		local near = if over and graph.index[over] then sim.neighbors(graph, over) else nil
		local z = v.zoom
		local function dim(a: number, lit: boolean): number
			return if near and not lit then a * 0.18 else a
		end
		local world = props.world()
		local loads = world.loads or {}
		local here = world.here

		-- The ground: a dot grid that moves with the view.
		local step = 32 * z
		while step < 18 do
			step *= 2
		end
		local grid = kit.ink("text.primary", 0.07)
		local gx0, gy0 = v.x % step, v.y % step
		local gy = gy0
		while gy < h do
			local gx = gx0
			while gx < w do
				c:circle(gx, gy, 0.9, grid)
				gx += step
			end
			gy += step
		end

		-- Islands: each machine's room, round everything it runs.
		local islands: { [string]: { x: number, y: number, r: number, top: boolean? } } = {}
		local inside: { [string]: boolean } = {}
		-- (Centred between the machine and what stays near it.)
		local members: { [string]: { sim.Node } } = {}
		for _, n in graph.nodes do
			if n.kind == "device" then
				islands[n.id] = { x = n.x, y = n.y, r = n.r + 30 }
				members[n.id] = {}
			end
		end
		for _, e in graph.edges do
			local isl = islands[e.a]
			local m = graph.nodes[graph.index[e.b]]
			if e.kind == "host" and isl and m and math.sqrt((m.x - isl.x) ^ 2 + (m.y - isl.y) ^ 2) + m.r <= ISLAND_MAX then
				table.insert(members[e.a], m)
				inside[`{e.a}>{e.b}`] = true
			end
		end
		for id, isl in islands do
			local list = members[id]
			local sx, sy, k = isl.x * 2, isl.y * 2, 2
			for _, m in list do
				sx += m.x
				sy += m.y
				k += 1
			end
			isl.x, isl.y = sx / k, sy / k
			local dev = graph.nodes[graph.index[id]]
			isl.r = math.max(dev.r + 30, math.sqrt((dev.x - isl.x) ^ 2 + (dev.y - isl.y) ^ 2) + dev.r + ISLAND_PAD)
			for _, m in list do
				isl.r = math.max(isl.r, math.sqrt((m.x - isl.x) ^ 2 + (m.y - isl.y) ^ 2) + m.r + ISLAND_PAD)
			end
		end
		local function device_of(n: sim.Node): any
			for _, dev in world.devices or {} do
				if dev.id == n.device then
					return dev
				end
			end
			return nil
		end
		for _, n in graph.nodes do
			if n.kind ~= "device" then
				continue
			end
			local isl = islands[n.id]
			local lit = near == nil or near[n.id] == true
			local x, y = isl.x * z + v.x, isl.y * z + v.y
			local r = isl.r * z
			c:circle(x, y, r, kit.ink("text.primary", dim(if over == n.id then 0.06 else 0.035, lit)))
			-- The rim, broken for the name (at the top, or the bottom where a
			-- leader's name would sit on it); the CPU runs along it.
			local name_w = c:measure_text(n.title, { size = 12, weight = 600 })
			local function crowded(ny: number): number
				local k = 0
				local x0, x1, y0, y1 = x - name_w / 2 - 6, x + name_w / 2 + 6, ny - 10, ny + 10
				for _, m in graph.nodes do
					if m.kind ~= "device" then
						local mx, my = screen(m, v)
						local mr = m.r * z + 4
						-- (A leader's name runs off to its right.)
						local reach = if m.kind == "thread" then mr else mr + 150
						if mx + reach > x0 and mx - mr < x1 and my + mr > y0 and my - mr < y1 then
							k += 1
						end
					end
				end
				return k
			end
			isl.top = crowded(y - r) <= crowded(y + r)
			local gap = math.deg(math.asin(math.clamp((name_w / 2 + 10) / r, 0, 0.95)))
			local from, sweep = (if isl.top then -90 else 90) + gap, 360 - gap * 2
			c:stroke(ui.path():arc(x, y, r, from, sweep), kit.ink("text.primary", dim(0.11, lit)), 1)
			local l = (loads :: any)[n.device or ""]
			local f = if l and l.cpu then math.clamp(l.cpu, 0, 1) else 0
			if f > 0.005 then
				local ink = ContextUsage.ink(f)
				c:stroke(ui.path():arc(x, y, r, from, sweep * f), kit.ink(string.sub(ink, 2), dim(0.9, lit)), 2, { cap = "round" })
			end
		end

		-- Lines: hosting only where a member strays off its island (or lit);
		-- leading as a gentle curve in the orchestrator's hue.
		local function curve(ax: number, ay: number, bx: number, by: number): any
			local mx, my = (ax + bx) / 2, (ay + by) / 2
			local dx, dy = bx - ax, by - ay
			return ui.path():move(ax, ay):quad(mx - dy * 0.12, my + dx * 0.12, bx, by)
		end
		for _, e in graph.edges do
			local a, b = graph.nodes[graph.index[e.a]], graph.nodes[graph.index[e.b]]
			local ax, ay = screen(a, v)
			local bx, by = screen(b, v)
			local lit = near ~= nil and (e.a == over or e.b == over)
			if e.kind == "host" then
				if lit or not inside[`{e.a}>{e.b}`] then
					c:line(ax, ay, bx, by, kit.ink("text.primary", dim(if lit then 0.3 else 0.12, lit)), 1, { dash = { 2, 3 } })
				end
			elseif e.kind == "control" then
				c:stroke(curve(ax, ay, bx, by), hue.canvas(e.hue, dim(if lit then 0.95 else 0.6, lit)), if lit then 2 else 1.5, { cap = "round" })
			elseif e.kind == "started" then
				c:stroke(curve(ax, ay, bx, by), hue.canvas(e.hue, dim(if lit then 0.9 else 0.55, lit)), 1.5, { cap = "round", dash = { 5, 4 } })
			else
				c:stroke(curve(ax, ay, bx, by), hue.canvas(e.hue, dim(if lit then 0.65 else 0.3, lit)), 1.25, { cap = "round", dash = { 1, 4 } })
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

		-- The nodes.
		local plate = kit.ink("background", 1)
		for _, n in graph.nodes do
			local lit = near == nil or near[n.id] == true
			local x, y = screen(n, v)
			local r = n.r * z
			if n.kind == "device" then
				-- The machine itself: a square plate (hardware), its mark.
				c:rect(x - r, y - r, r * 2, r * 2, kit.ink("background", dim(0.95, lit)), 7 * z)
				c:rect(x - r, y - r, r * 2, r * 2, kit.ink("text.primary", dim(0.08, lit)), 7 * z)
				c:stroke(ui.path():rect(x - r + 0.5, y - r + 0.5, r * 2 - 1, r * 2 - 1, 7 * z), kit.ink("text.primary", dim(0.22, lit)), 1)
				local d = device_of(n)
				glyph(c, MachineHead.icon(d), x, y, kit.ink("text.primary", dim(0.8, lit)))
				if n.device == here or (d and d.here) then
					c:circle(x + r - 1, y - r + 1, 4.5, plate)
					c:circle(x + r - 1, y - r + 1, 3, kit.ink("zeron.tone.info.ink", dim(1, lit)))
				end
			elseif n.kind == "orchestrator" then
				c:circle(x, y, r + 5 * z, plate)
				c:stroke(ui.path():arc(x, y, r + 4 * z, 0, 360), hue.canvas(n.hue, dim(0.45, lit)), 1.5)
				c:circle(x, y, r, hue.canvas(n.hue, dim(1, lit)))
			elseif n.kind == "brain" then
				-- (An orchestrator's size, quieter: a tinted diamond in a line
				-- of its hue, where theirs are solid.)
				local k = r * 1.15
				local function diamond(s: number): any
					return ui.path():move(x, y - s):line(x + s, y):line(x, y + s):line(x - s, y):close()
				end
				c:fill(diamond(k + 2), plate)
				c:fill(diamond(k), hue.canvas(n.hue, dim(0.28, lit)))
				c:stroke(diamond(k - 0.75), hue.canvas(n.hue, dim(0.9, lit)), 1.5, { join = "round" })
			else
				local st = state_of(n.id)
				local faded = st == "done" or st == "idle"
				local rr = if faded then r * 0.8 else r
				local ink
				if st == "working" or st == "waiting" then
					-- (Small inside Zeron's working ring, laid over it.)
					ink = kit.ink("text.primary", dim(0.9, lit))
					rr = r * 0.55
				elseif st == "input" or st == "approval" or st == "failed" then
					ink = kit.ink(`zeron.status.{st}`, dim(1, lit))
					c:circle(x, y, rr + 4 * z, kit.ink(`zeron.status.{st}`, dim(0.22, lit)))
				elseif st == "done" then
					ink = kit.ink("zeron.status.done", dim(0.75, lit))
				else
					ink = kit.ink("text.primary", dim(0.55, lit))
				end
				c:circle(x, y, rr + 2, plate)
				if n.crew then
					c:circle(x, y, rr, ink)
				else
					c:stroke(ui.path():arc(x, y, rr - 0.75, 0, 360), ink, 1.5)
				end
			end
		end

		-- Names. A machine's on its island's rim; an orchestrator's and the
		-- Brain dump's on a plate beside it; a thread's when it needs you or
		-- works (or close up, or lit), quieter.
		local function short(t: string, most: number): string
			if utf8.len(t) and (utf8.len(t) :: number) > most then
				return string.sub(t, 1, (utf8.offset(t, most) :: number) - 1) .. "…"
			end
			return t
		end
		-- Placed in turn (machines, then leaders, then threads at work, then
		-- the rest): each beside its node where it clears every node and
		-- every name placed before it, else left out (its card still names it).
		local taken: { { number } } = {}
		-- How much is in the way of a name there (0: nothing).
		local function crowd(x0: number, y0: number, x1: number, y1: number, own: string): number
			local k = 0
			for _, b in taken do
				if x0 < b[3] and x1 > b[1] and y0 < b[4] and y1 > b[2] then
					k += 2
				end
			end
			for _, m in graph.nodes do
				if m.id ~= own then
					local mx, my = screen(m, v)
					local mr = m.r * z + 2
					if mx + mr > x0 and mx - mr < x1 and my + mr > y0 and my - mr < y1 then
						k += 1
					end
				end
			end
			return k
		end
		local function label(n: sim.Node, t: string, opts: any, back: number, force: boolean)
			local x, y = screen(n, v)
			local r = n.r * z + (if n.kind == "thread" then 6 else 9 * z)
			local tw, th = c:measure_text(t, opts)
			local bw, bh = tw + 10, th + 4
			-- Right, left, below, above.
			local spots = {
				{ x + r, y - bh / 2 },
				{ x - r - bw, y - bh / 2 },
				{ x - bw / 2, y + r - 2 },
				{ x - bw / 2, y - r - bh + 2 },
			}
			local at: { number }, least = spots[1], math.huge
			for _, sp in spots do
				local k = crowd(sp[1], sp[2], sp[1] + bw, sp[2] + bh, n.id)
				if k < least then
					at, least = sp, k
				end
				if k == 0 then
					break
				end
			end
			if least > 0 and not force then
				return
			end
			table.insert(taken, { at[1], at[2], at[1] + bw, at[2] + bh })
			c:rect(at[1], at[2], bw, bh, kit.ink("background", back), bh / 2)
			opts.baseline = "middle"
			c:text(t, at[1] + 5, at[2] + bh / 2, opts)
		end
		for _, n in graph.nodes do
			if n.kind == "device" then
				local lit = near == nil or near[n.id] == true
				local isl = islands[n.id]
				local x, y = isl.x * z + v.x, (isl.y + (if isl.top then -isl.r else isl.r)) * z + v.y
				local o = { size = 12, weight = 600 }
				local tw, th = c:measure_text(n.title, o)
				table.insert(taken, { x - tw / 2 - 6, y - th / 2 - 2, x + tw / 2 + 6, y + th / 2 + 2 })
				c:text(n.title, x, y, { size = 12, weight = 600, color = kit.ink("text.primary", dim(0.82, lit)), align = "center", baseline = "middle" })
			end
		end
		for _, n in graph.nodes do
			if n.kind == "orchestrator" or n.kind == "brain" then
				local lit = near == nil or near[n.id] == true
				label(n, short(n.title, 28), { size = 12, weight = 600, color = kit.ink("text.primary", dim(0.95, lit)) }, dim(0.78, lit), true)
			end
		end
		for pass = 1, 2 do
			for _, n in graph.nodes do
				if n.kind ~= "thread" then
					continue
				end
				local lit = near == nil or near[n.id] == true
				local st = state_of(n.id)
				local busy = st == "working" or st == "input" or st == "approval" or st == "failed"
				local lit_here = near ~= nil and near[n.id] == true
				if (pass == 1) == (busy or lit_here) then
					label(n, short(n.title, 26), { size = 11, color = kit.ink("text.primary", dim(if busy then 0.78 else 0.55, lit)) }, dim(0.6, lit), lit_here)
				end
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
		local function doing(): string
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
		end
		local function running(): string
			local a = c().activity
			return if st() == "working" and a then a.label or "" else ""
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
			-- (What it's doing, and what it runs, where there's something to say.)
			ui.show(function(): boolean
				return doing() ~= ""
			end, ui.text { doing, text_size = "$font.size.xs", color = "$text.secondary" }),
			ui.show(function(): boolean
				return running() ~= ""
			end, ui.text {
				running,
				literal = true,
				truncate = true,
				w_full = true,
				font = "$font.mono",
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			}),
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
			-- (A plain scroll is the page's: it scrolls past the Map. Only
			-- ⌘/Ctrl-scroll zooms the web.)
			on_wheel = function(e: any): boolean
				if e.ctrl or e.cmd then
					moved_view = true
					zoom_at(e.local_x, e.local_y, math.exp(-e.dy * 0.01))
					return true
				end
				return false
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

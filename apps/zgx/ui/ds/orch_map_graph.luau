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
	menu: ((id: string) -> { any }?)?,
	register: ((key: string, open: ((id: string) -> boolean)?) -> ())?,
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
		local most = if #graph.nodes <= 12 then 1.1 else 1
		local zoom = math.clamp(math.min((w - 64) / math.max(1, x1 - x0), (h - 72) / math.max(1, y1 - y0)), 0.35, most)
		view:set({ x = w / 2 - (x0 + x1) / 2 * zoom, y = h / 2 - (y0 + y1) / 2 * zoom, zoom = zoom })
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

	-- Whether the theme is dark (its ink light): a few alphas differ.
	local function is_dark(): boolean
		local ink = kit.ink("text.primary")
		local r, g, b = tonumber(string.sub(ink, 2, 3), 16) or 0, tonumber(string.sub(ink, 4, 5), 16) or 0, tonumber(string.sub(ink, 6, 7), 16) or 0
		return r + g + b > 384
	end

	local function draw(c: any, w: number, h: number)
		tick:get()
		local v = view:get()
		local over = M.hover:get()
		local near = if over and graph.index[over] then sim.neighbors(graph, over) else nil
		local z = v.zoom
		-- (Marks keep near their size as the view zooms; the web's
		-- distances follow it.)
		local zs = math.clamp(z, 0.9, 1.25)
		local function dim(a: number, lit: boolean): number
			return if near and not lit then a * 0.18 else a
		end
		local dark = is_dark()
		local world = props.world()
		local loads = world.loads or {}
		local here = world.here
		local function ink(a: number, lit: boolean): string
			return kit.ink("text.primary", dim(a, lit))
		end
		local function at(n: sim.Node): (number, number)
			return n.x * z + v.x, n.y * z + v.y
		end
		local function lit_of(id: string): boolean
			return near == nil or near[id] == true
		end

		-- The ground: a dot grid that moves with the view.
		local step = 32 * z
		while step < 18 do
			step *= 2
		end
		local grid = kit.ink("text.primary", if dark then 0.06 else 0.1)
		local gy = v.y % step
		while gy < h do
			local gx = v.x % step
			while gx < w do
				c:circle(gx, gy, 0.8, grid)
				gx += step
			end
			gy += step
		end

		-- How far a node's mark reaches (a line stops short of it).
		local function reach(n: sim.Node): number
			if n.kind == "device" then
				return n.r * zs * 1.25
			elseif n.kind == "brain" then
				return n.r * zs * 1.15
			elseif n.kind == "thread" then
				return math.max(n.r * zs, 8)
			end
			return n.r * zs
		end

		-- 1. Where everything goes, nothing drawn yet: islands (each name
		-- in a gap in its rim, at the top or the bottom), lines, names.
		local islands = sim.islands(graph)
		local taken: { { number } } = {}
		local rims: { any } = {}
		local NAME = { size = 12, weight = 500 }
		-- (Each name outside its rim, on the side facing away from the
		-- rest, so the channels between islands stay clear.)
		local cx, cy, count = 0, 0, 0
		for _, isl in islands do
			cx += isl.x
			cy += isl.y
			count += 1
		end
		cx, cy = cx / math.max(1, count), cy / math.max(1, count)
		for _, n in graph.nodes do
			if n.kind ~= "device" then
				continue
			end
			local isl = islands[n.id]
			local x, y = isl.x * z + v.x, isl.y * z + v.y
			local r = isl.r * z
			local name_w, name_h = c:measure_text(n.title, NAME)
			local dx, dy = isl.x - cx, isl.y - cy
			local angle = if count <= 1 or dx * dx + dy * dy < 1 then -90 else math.deg(math.atan2(dy, dx))
			-- (Snapped to eight directions, so names line up.)
			angle = math.floor(angle / 45 + 0.5) * 45
			local ux, uy = math.cos(math.rad(angle)), math.sin(math.rad(angle))
			local px, py = x + ux * (r + 8), y + uy * (r + 8)
			-- Its box: beside the rim, growing outward.
			local x0 = if ux > 0.3 then px elseif ux < -0.3 then px - name_w else px - name_w / 2
			local y0 = if uy > 0.3 then py elseif uy < -0.3 then py - name_h else py - name_h / 2
			table.insert(taken, { x0 - 4, y0 - 2, x0 + name_w + 4, y0 + name_h + 2 })
			table.insert(rims, { n = n, x = x, y = y, r = r, nx = x0, ny = y0 + name_h / 2, from = -90, sweep = 360 })
		end

		local lines: { any } = {}
		for _, e in graph.edges do
			local a, b = graph.nodes[graph.index[e.a]], graph.nodes[graph.index[e.b]]
			local lit = near ~= nil and (e.a == over or e.b == over)
			if e.kind == "host" then
				-- (Shown only lit: the island says what runs where.)
				local isl = islands[e.a]
				if not lit and isl and isl.members[e.b] then
					continue
				end
			end
			local ax, ay = at(a)
			local bx, by = at(b)
			local dx, dy = bx - ax, by - ay
			local d = math.sqrt(dx * dx + dy * dy)
			local ra, rb = reach(a) + 3, reach(b) + 3
			if d <= ra + rb + 2 then
				continue
			end
			local ux, uy = dx / d, dy / d
			ax, ay, bx, by = ax + ux * ra, ay + uy * ra, bx - ux * rb, by - uy * rb
			-- (Each kind bowed its own way, so lines sharing a corridor
			-- run apart.)
			local bend = if e.kind == "host" then 0 elseif e.kind == "control" then 0.12 elseif e.kind == "started" then -0.16 else -0.26
			local mx, my = (ax + bx) / 2, (ay + by) / 2
			table.insert(lines, { e = e, lit = lit, ax = ax, ay = ay, bx = bx, by = by, cx = mx - (by - ay) * bend, cy = my + (bx - ax) * bend })
		end

		-- Names: leaders first, then threads at work or waiting on you, then
		-- the rest; each on the first side of its mark that clears every
		-- mark and name before it (a leader's on the least crowded); a
		-- thread's that can't is left to its card. Lines and rims break
		-- round them.
		-- A name in one line, or two split at a word near its middle (each
		-- at most `most` characters; past that, shortened).
		local function lines_of(t: string, most: number): { string }
			local n = utf8.len(t) or #t
			if n <= most then
				return { t }
			end
			local best, off = nil, math.huge
			for i in string.gmatch(t, "() ") do
				local d = math.abs(i - #t / 2)
				if d < off then
					best, off = i, d
				end
			end
			local function cut(x: string): string
				if (utf8.len(x) or #x) > most then
					return string.sub(x, 1, (utf8.offset(x, most) :: number) - 1) .. "…"
				end
				return x
			end
			if not best then
				return { cut(t) }
			end
			return { cut(string.sub(t, 1, best - 1)), cut(string.sub(t, best + 1)) }
		end
		-- Points along every line and rim, which a name would rather not cut.
		local pts: { { number } } = {}
		for _, ln in lines do
			local k = math.max(4, math.floor(math.sqrt((ln.bx - ln.ax) ^ 2 + (ln.by - ln.ay) ^ 2) / 8))
			for i = 0, k do
				local t = i / k
				local u = 1 - t
				table.insert(pts, { u * u * ln.ax + 2 * u * t * ln.cx + t * t * ln.bx, u * u * ln.ay + 2 * u * t * ln.cy + t * t * ln.by })
			end
		end
		for _, rim in rims do
			local k = math.max(12, math.floor(rim.r * math.rad(rim.sweep) / 8))
			for i = 0, k do
				local a = math.rad(rim.from + rim.sweep * i / k)
				table.insert(pts, { rim.x + math.cos(a) * rim.r, rim.y + math.sin(a) * rim.r })
			end
		end
		local function crowd(x0: number, y0: number, x1: number, y1: number, own: string): number
			local k = 0
			for _, p in pts do
				if p[1] > x0 and p[1] < x1 and p[2] > y0 and p[2] < y1 then
					k += 0.1
				end
			end
			k = math.min(k, 0.9)
			for _, bx in taken do
				if x0 < bx[3] and x1 > bx[1] and y0 < bx[4] and y1 > bx[2] then
					k += 1
				end
			end
			for _, m in graph.nodes do
				if m.id ~= own then
					local mx, my = at(m)
					local mr = reach(m) + 3
					if mx + mr > x0 and mx - mr < x1 and my + mr > y0 and my - mr < y1 then
						k += 1
					end
				end
			end
			if x0 < 4 or y0 < 4 or x1 > w - 4 or y1 > h - 4 then
				k += 1
			end
			return k
		end
		local labels: { any } = {}
		local function place(n: sim.Node, text: { string }, opts: any, force: boolean)
			local x, y = at(n)
			local r = (if n.kind == "thread" then 9 else reach(n)) + 7
			local tw, lh = 0, 0
			for _, t in text do
				local a, b = c:measure_text(t, opts)
				tw, lh = math.max(tw, a), math.max(lh, b)
			end
			local th = lh * #text
			-- Right, left (centred on the mark), then below, above.
			local spots = {
				{ x + r, y - th / 2, "start" },
				{ x - r - tw, y - th / 2, "end" },
				{ x - tw / 2, y + r - 3, "center" },
				{ x - tw / 2, y - r - th + 3, "center" },
			}
			local best: { any }, least = spots[1], math.huge
			for _, sp in spots do
				local k = crowd(sp[1] - 3, sp[2] - 1, sp[1] + tw + 3, sp[2] + th + 1, n.id)
				if k < least then
					best, least = sp, k
				end
				if k == 0 then
					break
				end
			end
			-- (Lines may run under a name, broken round it; marks and other
			-- names may not: a thread whose name fits nowhere keeps its dot.)
			if least >= 1 and not force then
				return
			end
			table.insert(taken, { best[1] - 3, best[2] - 1, best[1] + tw + 3, best[2] + th + 1 })
			for i, t in text do
				local o = table.clone(opts)
				o.baseline = "middle"
				o.align = best[3]
				local ax = if best[3] == "start" then best[1] elseif best[3] == "end" then best[1] + tw else best[1] + tw / 2
				table.insert(labels, { t = t, x = ax, y = best[2] + lh * (i - 0.5), opts = o })
			end
		end
		for _, n in graph.nodes do
			if n.kind == "orchestrator" or n.kind == "brain" then
				place(n, lines_of(n.title, 24), { size = 13, weight = 600, color = ink(if dark then 0.92 else 0.9, lit_of(n.id)) }, true)
			end
		end
		for pass = 1, 2 do
			for _, n in graph.nodes do
				if n.kind ~= "thread" then
					continue
				end
				local st = state_of(n.id)
				local busy = st == "working" or st == "input" or st == "approval" or st == "failed"
				local lit_here = near ~= nil and near[n.id] == true
				if (pass == 1) == (busy or lit_here) then
					local a = if busy then (if dark then 0.76 else 0.82) else (if dark then 0.52 else 0.64)
					place(n, lines_of(n.title, 22), { size = 12, color = ink(a, lit_of(n.id)) }, lit_here)
				end
			end
		end

		-- 2. Island grounds.
		for _, rim in rims do
			local lit = lit_of(rim.n.id)
			c:circle(rim.x, rim.y, rim.r, ink(if over == rim.n.id then 0.07 else (if dark then 0.045 else 0.04), lit))
		end

		-- 3. Rims and lines, broken round every name: the field, less a box
		-- a name (each traced the other way round, so it's left out).
		local field = ui.path():move(0, 0):line(w, 0):line(w, h):line(0, h):close()
		for _, bx in taken do
			field:move(bx[1], bx[2]):line(bx[1], bx[4]):line(bx[3], bx[4]):line(bx[3], bx[2]):close()
		end
		c:clip(field)
		for _, rim in rims do
			local lit = lit_of(rim.n.id)
			c:stroke(ui.path():arc(rim.x, rim.y, rim.r, rim.from, rim.sweep), ink(if dark then 0.16 else 0.22, lit), 1)
			local l = (loads :: any)[rim.n.device or ""]
			local f = if l and l.cpu then math.clamp(l.cpu, 0, 1) else 0
			if f > 0.005 then
				-- (Neutral, by its length; all of it a muted amber past 80%.)
				local paint = if f > 0.8
					then kit.ink(string.sub(ContextUsage.ink(f), 2), dim(if dark then 0.55 else 0.75, lit))
					else ink(if dark then 0.45 else 0.4, lit)
				c:stroke(ui.path():arc(rim.x, rim.y, rim.r, rim.from, rim.sweep * f), paint, 2, { cap = "round" })
			end
		end
		for _, ln in lines do
			local e, lit = ln.e, ln.lit
			local p = ui.path():move(ln.ax, ln.ay):quad(ln.cx, ln.cy, ln.bx, ln.by)
			if e.kind == "host" then
				c:stroke(p, ink(0.32, lit), 1, { dash = { 2, 3 } })
			elseif e.kind == "control" then
				c:stroke(p, hue.canvas(e.hue, dim(if lit then 0.95 else 0.6, lit)), if lit then 2 else 1.5, { cap = "round" })
			elseif e.kind == "started" then
				c:stroke(p, hue.canvas(e.hue, dim(if lit then 0.85 else 0.4, lit)), 1, { cap = "round", dash = { 4, 3 } })
			else
				c:stroke(p, hue.canvas(e.hue, dim(if lit then 0.7 else 0.35, lit)), 1.25, { cap = "round", dash = { 0.5, 3 } })
			end
		end
		-- Pulses along them.
		local now = ui.now()
		for _, p in pulses do
			local a, b = graph.nodes[graph.index[p.from] or 0], graph.nodes[graph.index[p.to] or 0]
			if a and b then
				local t = math.clamp((now - p.t0) / PULSE_MS, 0, 1)
				t = t * t * (3 - 2 * t)
				local ax, ay = at(a)
				local bx, by = at(b)
				local x, y = ax + (bx - ax) * t, ay + (by - ay) * t
				c:circle(x, y, 6, hue.canvas(p.hue, 0.25))
				c:circle(x, y, 2.75, hue.canvas(p.hue, 1))
			end
		end
		c:restore()

		-- 4. The marks.
		for _, n in graph.nodes do
			local lit = lit_of(n.id)
			local x, y = at(n)
			local r = n.r * zs
			if n.kind == "device" then
				-- The machine itself: a square plate (hardware), its mark.
				local rad = 6 * zs
				if dark then
					c:rect(x - r, y - r, r * 2, r * 2, kit.ink("background", dim(0.96, lit)), rad)
					c:rect(x - r, y - r, r * 2, r * 2, ink(0.05, lit), rad)
				end
				c:stroke(ui.path():rect(x - r + 0.6, y - r + 0.6, r * 2 - 1.2, r * 2 - 1.2, rad), ink(if dark then 0.22 else 0.35, lit), if dark then 1 else 1.25)
				local d = nil
				for _, dev in world.devices or {} do
					if dev.id == n.device then
						d = dev
					end
				end
				glyph(c, MachineHead.icon(d), x, y, ink(if dark then 0.75 else 0.7, lit))
				if n.device == here or (d and d.here) then
					c:circle(x + r - 1, y - r + 1, 4.5, kit.ink("background", 1))
					c:circle(x + r - 1, y - r + 1, 3, kit.ink("zeron.tone.info.ink", dim(1, lit)))
				end
			elseif n.kind == "orchestrator" then
				c:circle(x, y, r, hue.canvas(n.hue, dim(1, lit)))
			elseif n.kind == "brain" then
				-- (An orchestrator's size, quieter: tinted, in a line of its hue.)
				local k = r * 1.15
				local p = ui.path():move(x, y - k):line(x + k, y):line(x, y + k):line(x - k, y):close()
				c:fill(p, hue.canvas(n.hue, dim(0.3, lit)))
				local q = ui.path():move(x, y - k + 0.75):line(x + k - 0.75, y):line(x, y + k - 0.75):line(x - k + 0.75, y):close()
				c:stroke(q, hue.canvas(n.hue, dim(0.9, lit)), 1.5, { join = "round" })
			else
				local st = state_of(n.id)
				if st == "working" or st == "waiting" then
					-- (A dot inside Zeron's working ring, laid over it.)
					c:circle(x, y, 3 * zs, ink(0.9, lit))
				else
					local rr = r
					local paint
					if st == "input" or st == "approval" or st == "failed" then
						paint = kit.ink(`zeron.status.{st}`, dim(1, lit))
						c:circle(x, y, rr + 3.5 * zs, kit.ink(`zeron.status.{st}`, dim(0.2, lit)))
					elseif st == "done" then
						paint = kit.ink("zeron.status.done", dim(0.75, lit))
						rr = r * 0.85
					else
						paint = ink(0.45, lit)
						rr = r * 0.85
					end
					if n.crew then
						c:circle(x, y, rr, paint)
					else
						c:stroke(ui.path():arc(x, y, rr - 0.75, 0, 360), paint, 1.5)
					end
				end
			end
		end

		-- 5. The names.
		for _, rim in rims do
			c:text(rim.n.title, rim.nx, rim.ny, { size = 12, weight = 500, color = ink(if dark then 0.78 else 0.75, lit_of(rim.n.id)), baseline = "middle" })
		end
		for _, l in labels do
			c:text(l.t, l.x, l.y, l.opts)
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
				status.StatusIcon { kind = "working", size = 18 },
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
		-- A secondary click on a node: its menu (a machine's, "dev:<id>", too).
		kit.MenuField {
			menu = props.menu,
			register = props.register,
			key = "graph",
			hit = function(x: number, y: number): string?
				local n = hit(x, y)
				return if n then n.id else nil
			end,
			center = function(id: string): (number?, number?)
				local n = graph.nodes[graph.index[id] or 0]
				if not n then
					return nil, nil
				end
				return screen(n, view:peek())
			end,
			child = ui.div {
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
		},
	}
end)

return M

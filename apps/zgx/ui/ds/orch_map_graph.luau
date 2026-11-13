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
M.trace = ui.state("control_map_graph_settle", {} :: any)
M.zoom_by = function(_k: number) end
-- Where each pulse was drawn last, and how far that sat off its line.
M.pulse_trace = ui.state("control_map_graph_pulse", {} :: any)

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
	-- (How the settling went, for tests and tuning: frames from a pump's
	-- start till still, and how many times it was started.)
	local settle_frames = 0
	local function pump()
		run += 1
		local mine = run
		settled:set(false)
		settle_frames = 0
		M.trace:set({ frames = 0, runs = (M.trace:peek().runs or 0) + 1, done = false })
		local function frame()
			if mine ~= run then
				return
			end
			settle_frames += 1
			local moving = sim.step(graph)
			moving = sim.step(graph) or moving
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
				M.trace:set({ frames = settle_frames, runs = M.trace:peek().runs or 1, done = true })
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
			-- (The same web: only names may have moved, a machine renamed or
			-- a thread retitled. Taken in place, nothing moved.)
			local renamed = false
			for _, n in g.nodes do
				local i = graph.index[n.id]
				local old = i and graph.nodes[i]
				if old and old.title ~= n.title then
					old.title = n.title
					renamed = true
				end
			end
			if renamed then
				version:set(version:peek() + 1)
				tick:set(tick:peek() + 1)
			end
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
	-- Each machine's header tab as last drawn, by its node's id: x0, y0, x1, y1.
	local tabs: { [string]: { number } } = {}
	local function hit(px: number, py: number): sim.Node?
		local v = view:peek()
		-- (A machine is its tab.)
		for id, t in tabs do
			if px >= t[1] and px <= t[3] and py >= t[2] and py <= t[4] then
				local i = graph.index[id]
				if i then
					return graph.nodes[i]
				end
			end
		end
		local best, best_d = nil, math.huge
		for _, n in graph.nodes do
			if n.kind == "device" then
				continue
			end
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
		-- Each machine's heading: a tab on its island's rim (its mark, its
		-- name, its load), the rim broken behind it; at the top, else where
		-- it clears what's round it.
		local retab = false
		local TAB_NAME = { size = 13.5, weight = 600 }
		local TAB_LOAD = { size = 11, tabular = true }
		local TAB_H = 24
		for _, n in graph.nodes do
			if n.kind ~= "device" then
				continue
			end
			local isl = islands[n.id]
			local x, y = isl.x * z + v.x, isl.y * z + v.y
			local r = isl.r * z
			local d = nil
			for _, dev in world.devices or {} do
				if dev.id == n.device then
					d = dev
				end
			end
			local l = (loads :: any)[n.device or ""]
			local load = if l and l.cpu then `{math.floor(math.clamp(l.cpu, 0, 1) * 100 + 0.5)}%` else ""
			-- (A long name shortened: the heading never outgrows its island.)
			local title = n.title
			if (utf8.len(title) or #title) > 18 then
				title = string.sub(title, 1, (utf8.offset(title, 18) :: number) - 1) .. "…"
			end
			local name_w = c:measure_text(title, TAB_NAME)
			local load_w = if load ~= "" then c:measure_text(load, TAB_LOAD) else 0
			local tw = 8 + 14 + 6 + name_w + (if load ~= "" then 10 + load_w else 0) + 8
			-- (The island makes room for it, and an empty one for its one
			-- line saying so: settled again if either grew. Measured as at
			-- zoom 1, so a zoom — a view's change, not the web's — never
			-- moves anything.)
			local was = ((n :: any).tab_w or 0) + ((n :: any).idle_w or 0)
			;(n :: any).idle_w = if next(isl.members) == nil then c:measure_text(locale.s("ds.orch.map.idle"), { size = 11 }) else 0
			;(n :: any).tab_w = tw
			if math.abs(was - tw - (n :: any).idle_w) > 4 and not retab then
				retab = true
				ui.after(0, function()
					graph.alpha = math.max(graph.alpha, 0.3)
					pump()
				end)
			end
			table.insert(rims, { n = n, d = d, title = title, empty = next(isl.members) == nil, x = x, y = y, r = r, angle = -90, w = tw, h = TAB_H, name_w = name_w, load = load, nx = 0, ny = 0, from = -90, sweep = 360 })
		end
		-- (Centred on its rim at `angle`.)
		local function name_box(rim: any, angle: number): (number, number)
			local px, py = rim.x + math.cos(math.rad(angle)) * rim.r, rim.y + math.sin(math.rad(angle)) * rim.r
			return px - rim.w / 2, py - rim.h / 2
		end
		-- (Held for the working threads' names till the machine names go.)
		for _, rim in rims do
			local x0, y0 = name_box(rim, rim.angle)
			table.insert(taken, { x0 - 8, y0 - 10, x0 + rim.w + 8, y0 + rim.h + 10, rim = rim })
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
			-- (Bowed round any mark it doesn't join: its own bow first, then
			-- the other way, then wider.)
			local function hits(k: number): number
				local qx, qy = mx - (by - ay) * k, my + (bx - ax) * k
				local n_hit = 0
				for _, m in graph.nodes do
					if m.id ~= e.a and m.id ~= e.b then
						local px, py = at(m)
						local room = reach(m) + 6
						for i = 1, 11 do
							local t = i / 12
							local u = 1 - t
							local x = u * u * ax + 2 * u * t * qx + t * t * bx
							local y = u * u * ay + 2 * u * t * qy + t * t * by
							if (x - px) ^ 2 + (y - py) ^ 2 < room * room then
								n_hit += 1
								break
							end
						end
					end
				end
				return n_hit
			end
			if e.kind ~= "host" then
				local best, least = bend, hits(bend)
				if least > 0 then
					for _, k in { -bend, bend * 2, -bend * 2 } do
						local h = hits(k)
						if h < least then
							best, least = k, h
						end
						if h == 0 then
							break
						end
					end
				end
				bend = best
			end
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
		local rim_pts: { { number } } = {}
		for _, ln in lines do
			local k = math.max(4, math.floor(math.sqrt((ln.bx - ln.ax) ^ 2 + (ln.by - ln.ay) ^ 2) / 8))
			for i = 0, k do
				local t = i / k
				local u = 1 - t
				table.insert(pts, { u * u * ln.ax + 2 * u * t * ln.cx + t * t * ln.bx, u * u * ln.ay + 2 * u * t * ln.cy + t * t * ln.by, ln.e.a, ln.e.b } :: any)
			end
		end
		for _, rim in rims do
			local k = math.max(12, math.floor(rim.r * math.rad(rim.sweep) / 8))
			for i = 0, k do
				local a = math.rad(rim.from + rim.sweep * i / k)
				table.insert(rim_pts, { rim.x + math.cos(a) * rim.r, rim.y + math.sin(a) * rim.r })
			end
		end
		local function crowd(x0: number, y0: number, x1: number, y1: number, own: string, rim_w: number?): number
			local k = 0
			for _, p in pts do
				-- (Its own lines in are ranked apart (`on_own`), not counted
				-- against it here.)
				if p[1] > x0 and p[1] < x1 and p[2] > y0 and p[2] < y1 and (p :: any)[3] ~= own and (p :: any)[4] ~= own then
					k += 0.3
				end
			end
			k = math.min(k, 1.5)
			-- (A rim cut reads worse than a line's.)
			for _, p in rim_pts do
				if p[1] > x0 and p[1] < x1 and p[2] > y0 and p[2] < y1 then
					k += rim_w or 0.6
					break
				end
			end
			for _, bx in taken do
				if x0 < bx[3] and x1 > bx[1] and y0 < bx[4] and y1 > bx[2] then
					-- (A machine name not yet settled gives way.)
					k += if (bx :: any).rim then 0.5 else 1
				end
			end
			for _, m in graph.nodes do
				if m.id ~= own and m.kind ~= "device" then
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
		-- Whether a box sits on a line into `own` (its name over its own
		-- line in): a place to prefer against, never one that drops it.
		local own_pts: { [string]: { { number } } } = {}
		for _, ln in lines do
			for _, id in { ln.e.a, ln.e.b } do
				local list = own_pts[id] or {}
				own_pts[id] = list
				for i = 0, 12 do
					local t = i / 12
					local u = 1 - t
					table.insert(list, { u * u * ln.ax + 2 * u * t * ln.cx + t * t * ln.bx, u * u * ln.ay + 2 * u * t * ln.cy + t * t * ln.by })
				end
			end
		end
		local function on_own(x0: number, y0: number, x1: number, y1: number, own: string): number
			for _, p in own_pts[own] or {} do
				if p[1] > x0 and p[1] < x1 and p[2] > y0 and p[2] < y1 then
					return 0.5
				end
			end
			return 0
		end
		-- (A thread's name would rather stay on its own island.)
		local function outside(n: sim.Node, x0: number, y0: number, x1: number, y1: number): number
			if n.kind ~= "thread" then
				return 0
			end
			local isl = islands[`dev:{n.device or ""}`]
			if not isl then
				return 0
			end
			local ix, iy, ir = isl.x * z + v.x, isl.y * z + v.y, isl.r * z - 4
			for _, p in { { x0, y0 }, { x1, y0 }, { x0, y1 }, { x1, y1 } } do
				if (p[1] - ix) ^ 2 + (p[2] - iy) ^ 2 > ir * ir then
					return 0.35
				end
			end
			return 0
		end
		local labels: { any } = {}
		local leaders: { any } = {}
		-- A name's place: tried in one line, then two, on each side (right,
		-- left, centred on the mark; then below, above), and for a name that
		-- must show (a leader's, a working thread's) hanging under or over
		-- it and at its corners too.
		-- Whether a box sits on a rim (a name reads broken there).
		local function on_rim(x0: number, y0: number, x1: number, y1: number): number
			for _, p in rim_pts do
				if p[1] > x0 and p[1] < x1 and p[2] > y0 and p[2] < y1 then
					return 1
				end
			end
			return 0
		end
		-- Whether a box would sit on another name or a mark (never allowed).
		local function hard(x0: number, y0: number, x1: number, y1: number, own: string): boolean
			for _, bx in taken do
				if x0 < bx[3] and x1 > bx[1] and y0 < bx[4] and y1 > bx[2] then
					return true
				end
			end
			for _, m in graph.nodes do
				if m.id ~= own and m.kind ~= "device" then
					local mx, my = at(m)
					local mr = reach(m) + 3
					if mx + mr > x0 and mx - mr < x1 and my + mr > y0 and my - mr < y1 then
						return true
					end
				end
			end
			return false
		end
		-- A name's place: tried in one line, then two, on each side (right,
		-- left, centred on the mark; then below, above; hanging; the
		-- corners), close in, then a step out with a leader; shortened
		-- only when nothing else is free. Never on another name or a mark.
		local function place(n: sim.Node, forms: { { string } }, opts: any, must: boolean, force: boolean)
			local x, y = at(n)
			-- (Its mark's own radius: a name sits 7px off it, or a clear step
			-- out with a leader.)
			local st_m = if n.kind == "thread" then state_of(n.id) else ""
			local own_r = if st_m == "working" or st_m == "waiting" then 9 elseif n.kind == "thread" then n.r * zs else reach(n)
			local base = own_r + 7
			-- (The full name's forms, then a shortened one as a last resort.)
			local full = forms[#forms]
			local all_forms = table.clone(forms)
			local long = 0
			for _, t in full do
				long = math.max(long, utf8.len(t) or #t)
			end
			if long > 12 then
				-- (Shortened at the last whole word, or hyphen, that fits: 18
				-- characters, then 12; never a one-word stub while more fits.)
				local t = if #full == 1 then full[1] else table.concat(full, " ")
				for _, most in { 18, 12 } do
					if (utf8.len(t) or #t) > most then
						local head = string.sub(t, 1, (utf8.offset(t, most + 1) or (#t + 1)) - 1)
						local cut = string.match(head, "^(.*%S)[%s%-]+[^%s%-]*$") or head
						local short = cut .. "…"
						local dup = false
						for _, f in all_forms do
							if f[1] == short then
								dup = true
							end
						end
						if not dup then
							table.insert(all_forms, { short })
						end
					end
				end
			end
			local cands: { any } = {}
			for fi, text in all_forms do
				local short = fi > #forms
				for _, far in (if n.kind == "thread" then { 0, 16, 24 } else { 0 }) do
					local r = base + far
					local tw, lh = 0, 0
					for _, t in text do
						local aw, bh = c:measure_text(t, opts)
						tw, lh = math.max(tw, aw), math.max(lh, bh)
					end
					lh = math.floor(lh * 1.08 + 0.5)
					local th = lh * #text
					local spots = {
						{ x + r, y - th / 2, "start" },
						{ x - r - tw, y - th / 2, "end" },
						{ x - tw / 2, y + r - 1, "center" },
						{ x - tw / 2, y - r - th + 1, "center" },
						{ x - 8, y + r + 1, "start", true },
						{ x + 8 - tw, y + r + 1, "end", true },
						{ x - 8, y - r - th - 1, "start", true },
						{ x + 8 - tw, y - r - th - 1, "end", true },
					}
					local d = r * 0.72
					table.insert(spots, { x + d, y + d - th / 2, "start", true })
					table.insert(spots, { x + d, y - d - th / 2, "start", true })
					table.insert(spots, { x - d - tw, y + d - th / 2, "end", true })
					table.insert(spots, { x - d - tw, y - d - th / 2, "end", true })
					for si, sp in spots do
						local x0, y0, x1, y1 = sp[1], sp[2], sp[1] + tw, sp[2] + th
						-- (A leader's name to its right, else its left: an anchor.)
						local side_cost = if n.kind ~= "thread" and si > 2 then 3 else 0
						if x0 < 6 or y0 < 4 or x1 > w - 6 or y1 > h - 4 then
							continue
						end
						if hard(x0 - 4, y0 - 2, x1 + 4, y1 + 2, n.id) then
							continue
						end
						-- (Its own mark the nearest to it, and a leader, if any,
						-- clear of every other mark by 8px.)
						local qx, qy = math.clamp(x, x0, x1), math.clamp(y, y0, y1)
						local own_gap = math.sqrt((qx - x) ^ 2 + (qy - y) ^ 2) - own_r
						local wrong = false
						for _, m in graph.nodes do
							if m.id ~= n.id and m.kind ~= "device" then
								local mx, my = at(m)
								local mq_x, mq_y = math.clamp(mx, x0, x1), math.clamp(my, y0, y1)
								local mg = math.sqrt((mq_x - mx) ^ 2 + (mq_y - my) ^ 2) - reach(m)
								if mg < own_gap then
									wrong = true
									break
								end
								if far > 0 then
									-- (Distance from the mark to the leader's segment.)
									local dx, dy = qx - x, qy - y
									local l2 = math.max(1, dx * dx + dy * dy)
									local tt = math.clamp(((mx - x) * dx + (my - y) * dy) / l2, 0, 1)
									local px, py = x + dx * tt, y + dy * tt
									if math.sqrt((mx - px) ^ 2 + (my - py) ^ 2) < reach(m) + 8 then
										wrong = true
										break
									end
								end
							end
						end
						if wrong then
							continue
						end
						local k = crowd(x0 - 4, y0 - 4, x1 + 4, y1 + 4, n.id, if must and n.kind == "thread" then 0.35 else nil)
						local rank = k + side_cost + far * 0.03 + (if short then 3 * (fi - #forms) else 0) + on_own(x0 - 2, y0 - 2, x1 + 2, y1 + 2, n.id) + outside(n, x0, y0, x1, y1) + on_rim(x0 - 4, y0 - 4, x1 + 4, y1 + 4)
						table.insert(cands, { rank = rank, k = k, spot = { x = sp[1], y = sp[2], align = sp[3], tw = tw, th = th, lh = lh, text = text, aside = sp[4] == true or far > 0 } })
					end
				end
			end
			if #cands == 0 then
				return
			end
			table.sort(cands, function(p: any, q: any): boolean
				return p.rank < q.rank
			end)
			local best = cands[1].spot
			-- (Never past the field's edge: slid back in, a leader tying it.)
			local cx0 = math.clamp(best.x, 6, math.max(6, w - best.tw - 6))
			local cy0 = math.clamp(best.y, 4, math.max(4, h - best.th - 4))
			if cx0 ~= best.x or cy0 ~= best.y then
				best = table.clone(best)
				best.x, best.y, best.aside = cx0, cy0, true
			end
			table.insert(taken, { best.x - 3, best.y - 1, best.x + best.tw + 3, best.y + best.th + 1 })
			-- (Set off from its mark: a fine leader ties it back.)
			local st_n = if n.kind == "thread" then state_of(n.id) else ""
			local mr = if st_n == "working" or st_n == "waiting" then 9 elseif n.kind == "thread" then n.r * zs else reach(n)
			local qx = math.clamp(x, best.x, best.x + best.tw)
			local qy = math.clamp(y, best.y, best.y + best.th)
			local gx, gy = qx - x, qy - y
			local gd = math.sqrt(gx * gx + gy * gy)
			-- (Always for a name hung aside or stepped out; else past 13px.)
			-- (A clear one or none: under 20px it reads as debris.)
			if gd - mr >= 16 then
				table.insert(leaders, { x + gx / gd * (mr + 3), y + gy / gd * (mr + 3), qx - gx / gd * 3, qy - gy / gd * 3, lit_of(n.id) })
			end
			for i, t in best.text do
				local o = table.clone(opts)
				o.baseline = "middle"
				o.align = best.align
				local ax = if best.align == "start" then best.x elseif best.align == "end" then best.x + best.tw else best.x + best.tw / 2
				table.insert(labels, { t = t, x = ax, y = best.y + best.lh * (i - 0.5), opts = o })
			end
		end
		local function forms_of(t: string, most: number): { { string } }
			local one = lines_of(t, 40)
			local two = lines_of(t, most)
			if #two == 1 then
				return { two }
			end
			return if #one == 1 then { one, two } else { two }
		end
		for _, n in graph.nodes do
			if n.kind == "orchestrator" or n.kind == "brain" then
				-- (Under the machines' names: lighter, so a machine leads.)
				place(n, forms_of(n.title, 24), { size = 12.5, weight = 500, color = ink(if dark then 0.86 else 0.84, lit_of(n.id)) }, true, true)
			end
		end
		for pass = 1, 2 do
			if pass == 2 then
				-- The machine names, now: each where it was meant, else the
				-- nearest free place round its rim.
				local kept = {}
				for _, b in taken do
					if not (b :: any).rim then
						table.insert(kept, b)
					end
				end
				taken = kept
				tabs = {}
				for _, rim in rims do
					local best_a, least = -90, math.huge
					-- (A heading: at the top, or along the top of the rim where
					-- no line would pass under it; names round it give way.)
					-- (Lines pass under it; only marks and names move it, and
					-- then only a little way along the top.)
					-- (At the top where nothing crosses it, else along the top;
					-- if lines must, they pass under its backing.)
					for _, a in { -90, -70, -110, -50, -130 } do
						local x0, y0 = name_box(rim, a)
						local k = 0
						for _, p in pts do
							if p[1] > x0 - 8 and p[1] < x0 + rim.w + 8 and p[2] > y0 - 8 and p[2] < y0 + rim.h + 8 then
								k += 1
								break
							end
						end
						for _, o in rims do
							if o ~= rim then
								local nx = math.clamp(o.x, x0, x0 + rim.w)
								local ny = math.clamp(o.y, y0, y0 + rim.h)
								if math.abs(math.sqrt((nx - o.x) ^ 2 + (ny - o.y) ^ 2) - o.r) < 12 or (nx - o.x) ^ 2 + (ny - o.y) ^ 2 < o.r * o.r then
									k += 1
								end
							end
						end
						for _, m in graph.nodes do
							if m.kind ~= "device" then
								local mx, my = at(m)
								local mr = reach(m) + 4
								if mx + mr > x0 and mx - mr < x0 + rim.w and my + mr > y0 and my - mr < y0 + rim.h then
									k += 1
								end
							end
						end
						if k < 0.5 then
							best_a, least = a, k
							break
						elseif k < least then
							best_a, least = a, k
						end
					end
					local x0, y0 = name_box(rim, best_a)
					x0 = math.clamp(x0, 6, math.max(6, w - rim.w - 6))
					y0 = math.clamp(y0, 4, math.max(4, h - rim.h - 4))
					rim.nx, rim.ny = x0, y0
					-- (The rim breaks behind it; the load runs on from its edge.)
					local gap = math.deg(math.asin(math.clamp((rim.w / 2 + 8) / math.max(1, rim.r), 0, 0.99)))
					rim.from, rim.sweep = best_a + gap, 360 - gap * 2
					tabs[rim.n.id] = { x0, y0, x0 + rim.w, y0 + rim.h }
					table.insert(taken, { x0 - 8, y0 - 10, x0 + rim.w + 8, y0 + rim.h + 10, tab = true })
				end
			end
			for _, n in graph.nodes do
				if n.kind ~= "thread" then
					continue
				end
				local st = state_of(n.id)
				local busy = st == "working" or st == "waiting" or st == "input" or st == "approval" or st == "failed"
				local lit_here = near ~= nil and near[n.id] == true
				if (pass == 1) == (busy or lit_here) then
					local a = if busy then (if dark then 0.76 else 0.82) else (if dark then 0.52 else 0.64)
					place(n, forms_of(n.title, 22), { size = 12, color = ink(a, lit_of(n.id)) }, busy or lit_here, lit_here)
				end
			end
		end

		-- Lines bowed again, now round the names too, where a bow clears
		-- more (a name still in the way breaks the line round it).
		local function blocked(ln: any, k: number): number
			local mx, my = (ln.ax + ln.bx) / 2, (ln.ay + ln.by) / 2
			local qx, qy = mx - (ln.by - ln.ay) * k, my + (ln.bx - ln.ax) * k
			local n_hit = 0
			for i = 1, 15 do
				local t = i / 16
				local u = 1 - t
				local x = u * u * ln.ax + 2 * u * t * qx + t * t * ln.bx
				local y = u * u * ln.ay + 2 * u * t * qy + t * t * ln.by
				for _, b in taken do
					if x > b[1] and x < b[3] and y > b[2] and y < b[4] then
						n_hit += 1
					end
				end
				for _, m in graph.nodes do
					if m.id ~= ln.e.a and m.id ~= ln.e.b then
						local px, py = at(m)
						local room = reach(m) + 6
						if (x - px) ^ 2 + (y - py) ^ 2 < room * room then
							n_hit += 3
						end
					end
				end
			end
			return n_hit
		end
		for _, ln in lines do
			if ln.e.kind ~= "host" then
				local dx, dy = ln.bx - ln.ax, ln.by - ln.ay
				local len2 = math.max(1, dx * dx + dy * dy)
				local mx, my = (ln.ax + ln.bx) / 2, (ln.ay + ln.by) / 2
				local now_k = ((ln.cx - mx) * -dy + (ln.cy - my) * dx) / len2
				local best, least = now_k, blocked(ln, now_k)
				if least > 0 then
					-- (Gently: a line looping wide reads as something else.)
					for _, k in { -now_k, now_k * 1.8, -now_k * 1.8 } do
						if math.abs(k) <= 0.26 then
							local hcount = blocked(ln, k)
							if hcount < least then
								best, least = k, hcount
							end
						end
					end
				end
				ln.cx, ln.cy = mx - dy * best, my + dx * best
			end
		end
		-- A line a name still sits on goes round it: through a point just
		-- past a corner of the name, in two joined curves.
		local function quad_pts(ax: number, ay: number, cx: number, cy: number, bx: number, by: number, out: { { number } })
			for i = 1, 11 do
				local t = i / 12
				local u = 1 - t
				table.insert(out, { u * u * ax + 2 * u * t * cx + t * t * bx, u * u * ay + 2 * u * t * cy + t * t * by })
			end
		end
		local function cost(list: { { number } }, ln: any): number
			local k = 0
			for _, p in list do
				for _, b in taken do
					if p[1] > b[1] - 4 and p[1] < b[3] + 4 and p[2] > b[2] - 4 and p[2] < b[4] + 4 then
						k += 1
					end
				end
				for _, m in graph.nodes do
					if m.id ~= ln.e.a and m.id ~= ln.e.b then
						local px, py = at(m)
						local room = reach(m) + 6
						if (p[1] - px) ^ 2 + (p[2] - py) ^ 2 < room * room then
							k += 3
						end
					end
				end
			end
			return k
		end
		for _, ln in lines do
			if ln.e.kind == "host" then
				continue
			end
			local here_pts = {}
			quad_pts(ln.ax, ln.ay, ln.cx, ln.cy, ln.bx, ln.by, here_pts)
			local now_cost = cost(here_pts, ln)
			if now_cost == 0 then
				continue
			end
			-- (Every name in its way.)
			local hits: { { number } } = {}
			for _, b in taken do
				for _, p in here_pts do
					if p[1] > b[1] - 4 and p[1] < b[3] + 4 and p[2] > b[2] - 4 and p[2] < b[4] + 4 then
						table.insert(hits, b)
						break
					end
				end
			end
			if #hits == 0 then
				continue
			end
			local corners = {}
			for _, hit in hits do
				for _, cn in { { hit[1] - 10, hit[2] - 10 }, { hit[3] + 10, hit[2] - 10 }, { hit[1] - 10, hit[4] + 10 }, { hit[3] + 10, hit[4] + 10 } } do
					table.insert(corners, cn)
				end
			end
			local dx, dy = ln.bx - ln.ax, ln.by - ln.ay
			local len = math.sqrt(dx * dx + dy * dy)
			local tx, ty = dx / len, dy / len
			local best_cost, best = now_cost, nil
			for _, corner in corners do
				local wx, wy = corner[1], corner[2]
				-- (Only a point well along the way: no hook back.)
				local along = ((wx - ln.ax) * tx + (wy - ln.ay) * ty) / len
				local aside_d = math.abs((wx - ln.ax) * -ty + (wy - ln.ay) * tx)
				-- (A long dashed route may swing wider round a name.)
				local wide = if ln.e.kind == "control" then 0.35 else 0.45
				if along < 0.08 or along > 0.92 or aside_d > len * wide then
					continue
				end
				local l1 = math.sqrt((wx - ln.ax) ^ 2 + (wy - ln.ay) ^ 2)
				local l2 = math.sqrt((ln.bx - wx) ^ 2 + (ln.by - wy) ^ 2)
				-- (Smooth through the point: its tangent along the line.)
				local arm = math.min(l1, l2) * 0.4
				local c1x, c1y = wx - tx * arm, wy - ty * arm
				local c2x, c2y = wx + tx * arm, wy + ty * arm
				local list = {}
				quad_pts(ln.ax, ln.ay, c1x, c1y, wx, wy, list)
				quad_pts(wx, wy, c2x, c2y, ln.bx, ln.by, list)
				-- (A detour costs a little for its length.)
				local k = cost(list, ln) + (l1 + l2 - len) / (if ln.e.kind == "control" then 35 else 70)
				if k < best_cost then
					best_cost, best = k, { wx, wy, c1x, c1y, c2x, c2y }
				end
			end
			if best then
				ln.via = best
			end
		end

		-- Where a line meets a mark: the mark's own outline (a disc, the
		-- diamond, the machine's square, a working thread's ring), and a
		-- hair's gap.
		local leave: (q: { number }, n: sim.Node, at_end: boolean) -> number
		local sub_quad: (q: { number }, t0: number, t1: number) -> { number }
		local quad_length: (q: { number }) -> number
		local GAP = 2
		local function inside(n: sim.Node, px: number, py: number): boolean
			local x, y = at(n)
			local dx, dy = math.abs(px - x), math.abs(py - y)
			local r = n.r * zs
			if n.kind == "device" then
				local t = tabs[n.id]
				if t then
					return px >= t[1] - GAP and px <= t[3] + GAP and py >= t[2] - GAP and py <= t[4] + GAP
				end
				return dx <= r + GAP and dy <= r + GAP
			elseif n.kind == "brain" then
				return dx + dy <= r * 1.15 + GAP * 1.4
			elseif n.kind == "thread" then
				local st = state_of(n.id)
				local rr = if st == "working" or st == "waiting" then 9
					elseif st == "input" or st == "approval" or st == "failed" then r
					else r * 0.85
				return dx * dx + dy * dy <= (rr + GAP) ^ 2
			end
			return dx * dx + dy * dy <= (r + GAP) ^ 2
		end
		local function qpt(q: { number }, t: number): (number, number)
			local u = 1 - t
			return u * u * q[1] + 2 * u * t * q[3] + t * t * q[5], u * u * q[2] + 2 * u * t * q[4] + t * t * q[6]
		end
		-- The parameter where `q` crosses `n`'s outline: leaving it from the
		-- start, or (`at_end`) reaching it at the end.
		leave = function(q: { number }, n: sim.Node, at_end: boolean): number
			local lo, hi = if at_end then 0.5 else 0, if at_end then 1 else 0.5
			-- (Inside at one end, outside at the other: halve till close.)
			for _ = 1, 24 do
				local mid = (lo + hi) / 2
				local x, y = qpt(q, mid)
				local within = inside(n, x, y)
				if at_end then
					if within then
						hi = mid
					else
						lo = mid
					end
				else
					if within then
						lo = mid
					else
						hi = mid
					end
				end
			end
			return if at_end then lo else hi
		end
		-- The part of `q` from t0 to t1, as a curve of its own.
		sub_quad = function(q: { number }, t0: number, t1: number): { number }
			local x0, y0 = qpt(q, t0)
			local x1, y1 = qpt(q, t1)
			local function blossom(i: number): number
				return (1 - t0) * (1 - t1) * q[i] + ((1 - t0) * t1 + t0 * (1 - t1)) * q[i + 2] + t0 * t1 * q[i + 4]
			end
			return { x0, y0, blossom(1), blossom(2), x1, y1 }
		end
		quad_length = function(q: { number }): number
			local total, px, py = 0, q[1], q[2]
			for i = 1, 16 do
				local x, y = qpt(q, i / 16)
				total += math.sqrt((x - px) ^ 2 + (y - py) ^ 2)
				px, py = x, y
			end
			return total
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
			-- (A heading isn't cut out: lines run on under it, its backing
			-- laid over them.)
			if not (bx :: any).tab then
				field:move(bx[1], bx[2]):line(bx[1], bx[4]):line(bx[3], bx[4]):line(bx[3], bx[2]):close()
			end
		end
		-- (But round every mark a line reaches it whole: added back, traced
		-- the field's way.)
		for _, n in graph.nodes do
			local x, y = at(n)
			local rr = reach(n) + 5
			field:move(x + rr, y):arc(x, y, rr, 0, 360):close()
		end
		c:clip(field)
		-- (Rims too break round a name, never strike through it.)
		for _, rim in rims do
			local lit = lit_of(rim.n.id)
			c:stroke(ui.path():arc(rim.x, rim.y, rim.r, rim.from, rim.sweep), ink(if dark then 0.16 else 0.22, lit), 1, { cap = "round" })
		end
		c:restore()
		local paths: { [string]: { { number } } } = {}
		for _, ln in lines do
			local e, lit = ln.e, ln.lit
			local a, b = graph.nodes[graph.index[e.a]], graph.nodes[graph.index[e.b]]
			-- (From centre to centre, cut where it leaves each mark; a
			-- machine's centre its tab's.)
			local ax, ay = at(a)
			local bx, by = at(b)
			local ta = tabs[a.id]
			if ta then
				ax, ay = (ta[1] + ta[3]) / 2, (ta[2] + ta[4]) / 2
			end
			local segs
			if ln.via then
				local v1 = ln.via
				local s1 = { ax, ay, v1[3], v1[4], v1[1], v1[2] }
				local s2 = { v1[1], v1[2], v1[5], v1[6], bx, by }
				local t0 = leave(s1, a, false)
				local t1 = leave(s2, b, true)
				segs = { sub_quad(s1, t0, 1), sub_quad(s2, 0, t1) }
			else
				-- (Its bow kept, now about the centres.)
				local mx, my = (ln.ax + ln.bx) / 2, (ln.ay + ln.by) / 2
				local cx, cy = (ax + bx) / 2 + (ln.cx - mx), (ay + by) / 2 + (ln.cy - my)
				local q = { ax, ay, cx, cy, bx, by }
				local t0 = leave(q, a, false)
				local t1 = leave(q, b, true)
				if t1 <= t0 then
					continue
				end
				segs = { sub_quad(q, t0, t1) }
			end
			-- (Its whole path, mark edge to mark edge, kept for the pulses
			-- that travel it.)
			paths[`{e.a}>{e.b}`] = segs
			-- (Broken round each name it would cross, by hand: a piece left
			-- shorter than 10px is dropped, not left as a stub.)
			local ex, ey = at(a)
			local fx, fy = at(b)
			local near_a, near_b = reach(a) + 5, reach(b) + 5
			local function hidden(px: number, py: number): boolean
				if (px - ex) ^ 2 + (py - ey) ^ 2 < near_a * near_a or (px - fx) ^ 2 + (py - fy) ^ 2 < near_b * near_b then
					return false
				end
				for _, bx in taken do
					if not (bx :: any).tab and px > bx[1] and px < bx[3] and py > bx[2] and py < bx[4] then
						return true
					end
				end
				return false
			end
			local pieces: { { number } } = {}
			local length = 0
			for si, sg in segs do
				local N = 48
				local run0: number? = nil
				local function close(t1: number)
					if run0 then
						local piece = sub_quad(sg, run0 :: number, t1)
						local lp = quad_length(piece)
						local touches = (si == 1 and (run0 :: number) == 0) or (si == #segs and t1 == 1)
						if lp >= 10 or touches then
							table.insert(pieces, piece)
							length += lp
						end
						run0 = nil
					end
				end
				for i = 0, N do
					local t = i / N
					local u = 1 - t
					local px = u * u * sg[1] + 2 * u * t * sg[3] + t * t * sg[5]
					local py = u * u * sg[2] + 2 * u * t * sg[4] + t * t * sg[6]
					if hidden(px, py) then
						close(math.max(0, (i - 1) / N))
					elseif run0 == nil then
						run0 = t
					end
				end
				close(1)
			end
			if #pieces == 0 then
				continue
			end
			local p = ui.path()
			for _, pc in pieces do
				p:move(pc[1], pc[2]):quad(pc[3], pc[4], pc[5], pc[6])
			end
			-- (A dashed line begins and ends on a dash, its pattern fitted to
			-- its length.)
			local function fitted(on: number, off: number): { number }
				local period = on + off
				local n = math.max(1, math.floor((length - on) / period + 0.5))
				local k = (length - on) / (n * period)
				if k <= 0 then
					return { on, off }
				end
				return { on * k, off * k }
			end
			if e.kind == "host" then
				c:stroke(p, ink(0.32, lit), 1, { dash = fitted(2, 3) })
			elseif e.kind == "control" then
				c:stroke(p, hue.canvas(e.hue, dim(if lit then 0.95 else 0.6, lit)), if lit then 2 else 1.5, { cap = "round" })
			elseif e.kind == "started" then
				c:stroke(p, hue.canvas(e.hue, dim(if lit then 0.85 else 0.45, lit)), 1.25, { cap = "round", dash = fitted(4, 3) })
			else
				c:stroke(p, hue.canvas(e.hue, dim(if lit then 0.7 else 0.4, lit)), 1.25, { cap = "round", dash = fitted(0.5, 3) })
			end
		end
		-- Pulses along them: each down its line as drawn (its bow, its way
		-- round a name), from where it leaves one mark to where it meets the
		-- other; between two with no line, straight, edge to edge.
		local now = ui.now()
		local trace = {}
		for _, p in pulses do
			local a, b = graph.nodes[graph.index[p.from] or 0], graph.nodes[graph.index[p.to] or 0]
			if a and b then
				local t = math.clamp((now - p.t0) / PULSE_MS, 0, 1)
				t = t * t * (3 - 2 * t)
				local segs = paths[`{p.from}>{p.to}`]
				local back = false
				if not segs then
					segs = paths[`{p.to}>{p.from}`]
					back = segs ~= nil
				end
				if not segs then
					local ax, ay = at(a)
					local bx, by = at(b)
					local q = { ax, ay, (ax + bx) / 2, (ay + by) / 2, bx, by }
					local t0, t1 = leave(q, a, false), leave(q, b, true)
					segs = if t1 > t0 then { sub_quad(q, t0, t1) } else { q }
				end
				-- (By length along it, so it moves at an even pace through
				-- the bend and the detour.)
				local lens, total = {}, 0
				for i, sg in segs do
					lens[i] = quad_length(sg)
					total += lens[i]
				end
				local want = (if back then 1 - t else t) * total
				local x, y = segs[1][1], segs[1][2]
				for i, sg in segs do
					if want <= lens[i] or i == #segs then
						-- (Within this piece: its parameter for that length.)
						local lo, hi = 0, 1
						for _ = 1, 18 do
							local mid = (lo + hi) / 2
							if quad_length(sub_quad(sg, 0, mid)) < want then
								lo = mid
							else
								hi = mid
							end
						end
						local u = (lo + hi) / 2
						local w = 1 - u
						x = w * w * sg[1] + 2 * w * u * sg[3] + u * u * sg[5]
						y = w * w * sg[2] + 2 * w * u * sg[4] + u * u * sg[6]
						break
					end
					want -= lens[i]
				end
				-- (How far that sits off its path: for tests.)
				local off = math.huge
				for _, sg in segs do
					for k = 0, 64 do
						local u = k / 64
						local w = 1 - u
						local px = w * w * sg[1] + 2 * w * u * sg[3] + u * u * sg[5]
						local py = w * w * sg[2] + 2 * w * u * sg[4] + u * u * sg[6]
						off = math.min(off, math.sqrt((px - x) ^ 2 + (py - y) ^ 2))
					end
				end
				-- (And how far off the straight line between the two centres:
				-- a bowed path takes it off that.)
				local cax, cay = at(a)
				local cbx, cby = at(b)
				local ddx, ddy = cbx - cax, cby - cay
				local dl = math.max(1e-3, math.sqrt(ddx * ddx + ddy * ddy))
				local chord = math.abs((x - cax) * ddy - (y - cay) * ddx) / dl
				local from_a = math.sqrt((x - cax) ^ 2 + (y - cay) ^ 2)
				table.insert(trace, { x = x, y = y, t = t, off = off, chord = chord, from_a = from_a, reach_a = reach(a), reach_b = reach(b), from_b = math.sqrt((x - cbx) ^ 2 + (y - cby) ^ 2) })
				c:circle(x, y, 6, hue.canvas(p.hue, 0.25))
				c:circle(x, y, 2.75, hue.canvas(p.hue, 1))
			end
		end
		if #trace > 0 or #(M.pulse_trace:peek() or {}) > 0 then
			ui.after(0, function()
				M.pulse_trace:set(trace)
			end)
		end

		-- 4. The marks.
		for _, n in graph.nodes do
			local lit = lit_of(n.id)
			local x, y = at(n)
			local r = n.r * zs
			if n.kind == "device" then
				-- (The machine is its tab, drawn with the names.)
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

		-- 5. The names, and the leaders of those set off.
		for _, l in leaders do
			-- (Faint and short: never to be read as a link between agents.)
			c:line(l[1], l[2], l[3], l[4], ink(if dark then 0.26 else 0.3, l[5]), 1, { cap = "round" })
		end
		for _, rim in rims do
			local lit = lit_of(rim.n.id)
			local x0, y0, tw, th = rim.nx, rim.ny, rim.w, rim.h
			local cy = y0 + th / 2
			-- (A soft backing, the field's own colour, so lines pass under;
			-- a hairline round it, the rim broken behind it.)
			c:rect(x0, y0, tw, th, kit.ink("background", dim(if dark then 0.94 else 0.62, lit)), 6)
			if over == rim.n.id then
				c:rect(x0, y0, tw, th, ink(0.06, lit), 6)
			end
			c:stroke(ui.path():rect(x0 + 0.5, y0 + 0.5, tw - 1, th - 1, 6), ink(if dark then 0.12 else 0.16, lit), 1)
			local gx = x0 + 8 + 7
			glyph(c, MachineHead.icon(rim.d), gx, cy, ink(if dark then 0.8 else 0.75, lit))
			if rim.n.device == here or (rim.d and rim.d.here) then
				c:circle(gx + 7, cy - 6, 3.5, kit.ink("background", 1))
				c:circle(gx + 7, cy - 6, 2.5, kit.ink("zeron.tone.info.ink", dim(1, lit)))
			end
			local nx = x0 + 8 + 14 + 6
			c:text(rim.title, nx, cy, { size = 13.5, weight = 600, color = ink(if dark then 0.95 else 0.9, lit), baseline = "middle" })
			if rim.load ~= "" then
				local l = (loads :: any)[rim.n.device or ""]
				local f = if l and l.cpu then l.cpu else 0
				-- (Its figure amber past 80%: the one sign a machine is full.)
				local paint = if f > 0.8 then kit.ink(string.sub(ContextUsage.ink(f), 2), dim(0.9, lit)) else ink(if dark then 0.5 else 0.5, lit)
				c:text(rim.load, nx + rim.name_w + 10, cy, { size = 11, tabular = true, color = paint, baseline = "middle" })
			end
			-- A machine with nothing at work says so, quietly, inside.
			if rim.empty then
				-- (One line: the island is sized for it.)
				c:text(locale.s("ds.orch.map.idle"), rim.x, rim.y + 6, { size = 11, color = ink(if dark then 0.4 else 0.42, lit), align = "center", baseline = "middle" })
			end
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
	-- (A zoom as a ⌘/Ctrl-scroll gives it, about the field's middle: for
	-- tests and fixtures.)
	M.zoom_by = function(k: number)
		moved_view = true
		zoom_at(width:peek() / 2, height() / 2, k)
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
				local t = tabs[x.id]
				if t then
					-- (A machine's: under its tab.)
					return math.clamp(t[1], 8, width:get() - 256)
				end
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
				local t = tabs[x.id]
				if t then
					return if t[4] + 8 > height() - 130 then math.max(8, t[2] - 140) else t[4] + 8
				end
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
				-- (A machine's: its tab.)
				local t = tabs[id]
				if t then
					return (t[1] + t[3]) / 2, (t[2] + t[4]) / 2
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
				-- (Held where it was taken: a machine by its tab moves its island.)
				local ox, oy = 0, 0
				if n then
					local sx, sy = screen(n, view:peek())
					ox, oy = sx - e.local_x, sy - e.local_y
				end
				press = { id = n and n.id, x = e.local_x, y = e.local_y, ox = ox, oy = oy, moved = false, view = view:peek() }
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
							n.x, n.y = to_world(e.local_x + press.ox, e.local_y + press.oy)
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

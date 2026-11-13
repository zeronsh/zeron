-- The Map's Graph (ds/orch_map_graph), worked out: its nodes (machines,
-- orchestrators, the Brain dump, threads) and edges (what runs where, what
-- leads what, what the Brain dump started or handed a part to), and a
-- force layout that settles them into a web: each machine with what runs
-- on it gathered round it, orchestrators drawing their crew in, across
-- machines too. Pure, and cheap per step (no work once settled).
--
--   sim.build(world)              Graph (below), seeded where it would settle
--   sim.carry(graph, from)        positions kept from an older graph
--   sim.step(graph)               one step; whether it still moves
--   sim.settle(graph, steps)      steps until still (or `steps`)
--   sim.bounds(graph)             x0, y0, x1, y1 of every node with its room
--   sim.neighbors(graph, id)      the ids next to it, itself included

local model = require("@ds/orch_map_model")

local M = {}

export type Node = {
	id: string,
	kind: string, -- "device" | "orchestrator" | "brain" | "thread"
	title: string,
	device: string?,
	hue: number?,
	-- A thread: one of an orchestrator's crew (or started by the Brain
	-- dump), else the person's own.
	crew: boolean,
	r: number,
	x: number,
	y: number,
	vx: number,
	vy: number,
	-- Held where it was dropped.
	fixed: boolean,
}
export type Edge = {
	a: string,
	b: string,
	-- "host": runs on; "control": leads (orchestrator → crew); "started":
	-- the Brain dump started it; "routed": the Brain dump handed it a part.
	kind: string,
	hue: number?,
	len: number,
	strength: number,
}
export type Graph = {
	nodes: { Node },
	edges: { Edge },
	index: { [string]: number },
	alpha: number,
}

M.R = { device = 16, orchestrator = 10, brain = 9, thread = 5 }
-- An island (a machine's room) reaches PAD past its farthest member;
-- islands keep GAP apart, rim to rim.
M.ISLAND_PAD, M.ISLAND_GAP = 28, 72
M.ALPHA_MIN = 0.004
local DECAY = 0.028
local FRICTION = 0.58

-- A steady number for a string (a stable angle for a new node).
local function hash(s: string): number
	local h = 0
	for i = 1, #s do
		h = (h * 31 + string.byte(s, i)) % 1000003
	end
	return h
end

function M.build(world: any): Graph
	local fleet = model.fleet(world)
	local g: Graph = { nodes = {}, edges = {}, index = {}, alpha = 1 }
	local function add(n: Node)
		table.insert(g.nodes, n)
		g.index[n.id] = #g.nodes
	end
	local function node(id: string, kind: string, title: string, device: string?, hue: number?, crew: boolean): Node
		return { id = id, kind = kind, title = title, device = device, hue = hue, crew = crew, r = (M.R :: any)[kind], x = 0, y = 0, vx = 0, vy = 0, fixed = false }
	end
	-- Machines on a ring, what runs on each round it.
	local n_dev = #fleet.machines
	for i, m in fleet.machines do
		local d = node(`dev:{m.id}`, "device", m.name, m.id, nil, false)
		local a = (i - 1) / math.max(1, n_dev) * math.pi * 2 - math.pi / 2
		local ring = if n_dev > 1 then 200 + 40 * n_dev else 0
		d.x, d.y = math.cos(a) * ring, math.sin(a) * ring
		add(d)
	end
	for _, t in fleet.threads do
		local o = fleet.orchestrators[t.id]
		local kind = if t.kind == "orchestrator" then (if o and o.kind == "brain_dump" then "brain" else "orchestrator") else "thread"
		local n = node(t.id, kind, t.title, t.device, t.hue, t.kind == "crew")
		local home = g.nodes[g.index[`dev:{t.device or ""}`] or 0]
		local a = (hash(t.id) % 628) / 100
		local dist = if kind == "thread" then 70 else 50
		n.x = (if home then home.x else 0) + math.cos(a) * dist
		n.y = (if home then home.y else 0) + math.sin(a) * dist
		add(n)
	end
	local function link(a: string, b: string, kind: string, hue: number?, len: number, strength: number)
		if g.index[a] and g.index[b] and a ~= b then
			table.insert(g.edges, { a = a, b = b, kind = kind, hue = hue, len = len, strength = strength })
		end
	end
	-- What runs where.
	for _, t in fleet.threads do
		local big = t.kind == "orchestrator"
		link(`dev:{t.device or ""}`, t.id, "host", nil, if big then 66 else 58, 0.9)
	end
	-- What leads what; what the Brain dump started; what it handed parts.
	local prov = model.provenance(world)
	for _, t in fleet.threads do
		if t.kind == "crew" and t.orchestrator then
			-- (Across machines, a looser pull: each stays on its island.)
			local lead = fleet.threads[t.orchestrator :: string]
			local away = if lead and lead.device ~= t.device then 0.1 else 1
			if t.started then
				link(t.orchestrator :: string, t.id, "started", t.hue, 110, 0.22 * away)
			else
				link(t.orchestrator :: string, t.id, "control", t.hue, 78, 0.45 * away)
			end
		end
	end
	for id, by in prov.started do
		local t = fleet.threads[id]
		if t and t.kind == "orchestrator" then
			local o = fleet.orchestrators[by]
			link(by, id, "started", o and o.hue, 110, 0.05)
		end
	end
	local seen: { [string]: boolean } = {}
	for _, e in g.edges do
		seen[`{e.a}>{e.b}`] = true
	end
	for _, r in prov.routed do
		if not seen[`{r.from}>{r.to}`] then
			local o = fleet.orchestrators[r.from]
			link(r.from, r.to, "routed", o and o.hue, 150, 0.04)
		end
	end
	return g
end

-- Positions (and pins) kept from `from` for the nodes both have; a new
-- node starts by a neighbor it has there.
function M.carry(g: Graph, from: Graph?)
	if not from then
		return
	end
	for _, n in g.nodes do
		local i = from.index[n.id]
		if i then
			local o = from.nodes[i]
			n.x, n.y, n.fixed = o.x, o.y, o.fixed
		else
			for _, e in g.edges do
				local other = if e.a == n.id then e.b elseif e.b == n.id then e.a else nil
				local j = other and from.index[other]
				if j then
					local o = from.nodes[j]
					local a = (hash(n.id) % 628) / 100
					n.x, n.y = o.x + math.cos(a) * 30, o.y + math.sin(a) * 30
					break
				end
			end
		end
	end
end

-- One step of the forces: springs along the edges, every node pushing
-- every other off (machines most), a pull to the middle, no overlaps.
-- Whether anything still moves.
function M.step(g: Graph): boolean
	local alpha = g.alpha
	if alpha < M.ALPHA_MIN then
		return false
	end
	local nodes = g.nodes
	local n = #nodes
	-- Springs.
	for _, e in g.edges do
		local a, b = nodes[g.index[e.a]], nodes[g.index[e.b]]
		local dx, dy = b.x + b.vx - a.x - a.vx, b.y + b.vy - a.y - a.vy
		local d = math.sqrt(dx * dx + dy * dy)
		if d < 1e-3 then
			dx, dy, d = 0.01, 0.01, 0.0141
		end
		local f = (d - e.len) / d * alpha * e.strength
		dx, dy = dx * f, dy * f
		b.vx -= dx * 0.5
		b.vy -= dy * 0.5
		a.vx += dx * 0.5
		a.vy += dy * 0.5
	end
	-- Each pushes each off, and none overlap.
	for i = 1, n do
		local a = nodes[i]
		local qa = if a.kind == "device" then 7000 elseif a.kind == "thread" then 300 else 1100
		for j = i + 1, n do
			local b = nodes[j]
			local qb = if b.kind == "device" then 7000 elseif b.kind == "thread" then 300 else 1100
			local dx, dy = b.x - a.x, b.y - a.y
			local d2 = dx * dx + dy * dy
			if d2 < 1e-4 then
				dx, dy, d2 = 0.1 * (i - j), 0.1, 0.01 * (i - j) * (i - j) + 0.01
			end
			if d2 < 640000 then
				local d = math.sqrt(d2)
				local f = math.sqrt(qa * qb) * alpha / d2
				local fx, fy = dx / d * f, dy / d * f
				a.vx -= fx
				a.vy -= fy
				b.vx += fx
				b.vy += fy
				-- (Room for a name beside a node that always shows one.)
				local room = a.r + b.r + 16 + (if a.kind ~= "thread" then 24 else 0) + (if b.kind ~= "thread" then 24 else 0)
				if d < room then
					local push = (room - d) / d * 0.5
					a.vx -= dx * push * 0.5
					a.vy -= dy * push * 0.5
					b.vx += dx * push * 0.5
					b.vy += dy * push * 0.5
				end
			end
		end
	end
	-- To the middle, gently; then each moves, slowed.
	for _, a in nodes do
		a.vx -= a.x * 0.012 * alpha
		a.vy -= a.y * 0.012 * alpha
		if a.fixed then
			a.vx, a.vy = 0, 0
		else
			a.vx *= FRICTION
			a.vy *= FRICTION
			a.x += a.vx
			a.y += a.vy
		end
	end
	-- Islands apart, rim to rim, each moved whole (a dropped node stays).
	local isl = M.islands(g)
	local ids = {}
	for id in isl do
		table.insert(ids, id)
	end
	table.sort(ids)
	local function shift(id: string, dx: number, dy: number)
		local d = nodes[g.index[id]]
		if not d.fixed then
			d.x += dx
			d.y += dy
		end
		for m in isl[id].members do
			local o = nodes[g.index[m]]
			if not o.fixed then
				o.x += dx
				o.y += dy
			end
		end
	end
	for i = 1, #ids do
		for j = i + 1, #ids do
			local p, q = isl[ids[i]], isl[ids[j]]
			local dx, dy = q.x - p.x, q.y - p.y
			local d = math.sqrt(dx * dx + dy * dy)
			local room = p.r + q.r + M.ISLAND_GAP
			if d < room then
				if d < 1e-3 then
					dx, dy, d = 1, 0, 1
				end
				local push = (room - d) / d * 0.25
				shift(ids[i], -dx * push, -dy * push)
				shift(ids[j], dx * push, dy * push)
			end
		end
	end
	g.alpha = alpha * (1 - DECAY)
	return g.alpha >= M.ALPHA_MIN
end

function M.settle(g: Graph, steps: number)
	for _ = 1, steps do
		if not M.step(g) then
			return
		end
	end
end

-- Each machine's island, by its node's id: round the machine and all it
-- runs, centred between them (the machine counted twice); `members` the
-- ids on it.
export type Island = { x: number, y: number, r: number, members: { [string]: boolean } }
function M.islands(g: Graph): { [string]: Island }
	local out: { [string]: Island } = {}
	local list: { [string]: { Node } } = {}
	for _, n in g.nodes do
		if n.kind == "device" then
			out[n.id] = { x = n.x, y = n.y, r = 0, members = {} }
			list[n.id] = { n, n }
		end
	end
	for _, n in g.nodes do
		local home = n.kind ~= "device" and list[`dev:{n.device or ""}`]
		if home then
			table.insert(home, n)
			out[`dev:{n.device or ""}`].members[n.id] = true
		end
	end
	for id, isl in out do
		local sx, sy = 0, 0
		for _, n in list[id] do
			sx += n.x
			sy += n.y
		end
		isl.x, isl.y = sx / #list[id], sy / #list[id]
		local r = 0
		for _, n in list[id] do
			r = math.max(r, math.sqrt((n.x - isl.x) ^ 2 + (n.y - isl.y) ^ 2) + n.r)
		end
		isl.r = math.max(r + M.ISLAND_PAD, list[id][1].r + 34)
	end
	return out
end

function M.bounds(g: Graph): (number, number, number, number)
	local x0: number, y0: number, x1: number, y1: number = math.huge, math.huge, -math.huge, -math.huge
	for _, n in g.nodes do
		x0, y0 = math.min(x0, n.x - n.r), math.min(y0, n.y - n.r)
		x1, y1 = math.max(x1, n.x + n.r), math.max(y1, n.y + n.r)
	end
	for _, isl in M.islands(g) do
		x0, y0 = math.min(x0, isl.x - isl.r), math.min(y0, isl.y - isl.r)
		x1, y1 = math.max(x1, isl.x + isl.r), math.max(y1, isl.y + isl.r)
	end
	if x0 > x1 then
		return -100, -100, 100, 100
	end
	return x0 :: number, y0 :: number, x1 :: number, y1 :: number
end

function M.neighbors(g: Graph, id: string): { [string]: boolean }
	local out: { [string]: boolean } = { [id] = true }
	for _, e in g.edges do
		if e.a == id then
			out[e.b] = true
		elseif e.b == id then
			out[e.a] = true
		end
	end
	return out
end

return M

--!native
--!optimize 2
-- The force Graph's physics (ds/orch_force), pure: no requires, no clock,
-- no randomness but its own seeded one. Obsidian-like: strong links are
-- stiff, short springs; weak links soft, long ones; every node pushes its
-- near neighbours away (a uniform grid, so a step is O(n), never n²) and
-- all drift gently to the middle. A step moves only the nodes awake (all,
-- on a cold start; a changed node and its neighbours, after a change; the
-- one dragged and its neighbours), the rest held where they are.
--
--   sim.new(nodes, links, seed?)   Sim: nodes { id, parent?, mass?, r? },
--                                  links { a, b, strong } (indices)
--   sim.place(s, saved)            positions: saved ones where known ({ [id]
--                                  = { x, y } }), else near the parent (or on
--                                  a ring round the middle); returns how many
--                                  were new
--   sim.wake(s, ids?)              wake these and their neighbours (all, nil)
--   sim.step(s)                    one step; its energy (sum of v²)
--   sim.run(s, budget_s, clock)    steps until still or `budget_s` spent
--                                  (`clock()` in seconds); whether still
--   sim.still(s)                   at rest
--   sim.positions(s)               { [id] = { x, y } }, to keep

local M = {}

-- Strong: an orchestrator to the Brain dump, a chat to its orchestrator.
M.STRONG_LEN, M.STRONG_K = 64, 0.22
-- (An orchestrator's link to the Brain dump, longer: the hubs spread.)
M.HUB_LEN = 190
-- Weak: chats that talk or command each other.
M.WEAK_LEN, M.WEAK_K = 220, 0.006
M.REPEL = 1800 -- strength of the push between nodes
M.REPEL_R = 90 -- past this, no push (the grid's cell)
M.CENTER = 0.004
M.DAMP = 0.55
M.MAX_V = 24
-- At rest: the energy per node awake below this.
-- (Under a third of a point a step, on average: still to the eye.)
M.REST = 0.1
M.MAX_STEPS = 400

-- (`fixed`: held where it is, the middle: the Brain dump.)
export type Node = { id: string, parent: number?, mass: number?, fixed: boolean? }
-- (`len`: its rest length, set from how many share the parent: a crew
-- rests on a ring as wide as it needs.)
export type Link = { a: number, b: number, strong: boolean, hub: boolean?, len: number? }

export type Sim = {
	n: number,
	ids: { string },
	index: { [string]: number },
	parent: { number },
	mass: { number },
	x: { number },
	y: { number },
	vx: { number },
	vy: { number },
	awake: { boolean },
	held: { boolean }, -- placed by the pointer (dragged), or fixed
	fresh: { boolean }, -- placed new (no place known for it)
	fixed: { boolean }, -- the middle, always held
	links: { Link },
	nbrs: { { number } },
	seed: number,
	steps: number,
	energy: number,
	-- The grid: cell key → first node, then next per node (no tables made
	-- per step).
	cells: { [number]: number },
	next_in: { number },
}

-- A steady number in [0, 1) from a string and a seed.
-- A ring round a parent wide enough for `count` children (a node's room
-- apart), at least `len`.
function M.ring(count: number, len: number): number
	return math.max(len, count * 15 / (2 * math.pi))
end

local function hash01(s: string, seed: number): number
	local h = seed % 2147483647
	for i = 1, #s do
		h = (h * 31 + string.byte(s, i)) % 2147483647
	end
	h = (h * 1103515245 + 12345) % 2147483648
	return h / 2147483648
end
M.hash01 = hash01

function M.new(nodes: { Node }, links: { Link }, seed: number?): Sim
	local n = #nodes
	local s: Sim = {
		n = n,
		ids = table.create(n) :: any,
		index = {},
		parent = table.create(n, 0),
		mass = table.create(n, 1),
		x = table.create(n, 0),
		y = table.create(n, 0),
		vx = table.create(n, 0),
		vy = table.create(n, 0),
		awake = table.create(n, true),
		held = table.create(n, false),
		fixed = {},
		fresh = table.create(n, false),
		links = links,
		nbrs = table.create(n) :: any,
		seed = seed or 7,
		steps = 0,
		energy = math.huge,
		cells = {},
		next_in = table.create(n, 0),
	}
	for i, nd in nodes do
		s.ids[i] = nd.id
		s.index[nd.id] = i
		s.parent[i] = nd.parent or 0
		s.mass[i] = nd.mass or 1
		s.nbrs[i] = {}
		s.held[i] = nd.fixed == true
	end
	s.fixed = table.create(n, false)
	for i, nd in nodes do
		s.fixed[i] = nd.fixed == true
	end
	for _, l in links do
		table.insert(s.nbrs[l.a], l.b)
		table.insert(s.nbrs[l.b], l.a)
	end
	local kids = table.create(n, 0)
	for i = 1, n do
		local p = s.parent[i]
		if p > 0 then
			kids[p] += 1
		end
	end
	-- (A hub's mass grows with its crew: many stiff springs on one light
	-- node would make it ring, never rest.)
	for i = 1, n do
		s.mass[i] = math.max(s.mass[i], 1 + kids[i] * M.STRONG_K * 1.5)
	end
	for _, l in links do
		if l.hub then
			l.len = M.HUB_LEN + M.ring(kids[l.b], M.STRONG_LEN)
		elseif l.strong then
			l.len = M.ring(kids[l.a], M.STRONG_LEN)
		else
			l.len = M.WEAK_LEN
		end
	end
	return s
end

-- Where everything starts: kept places first; then, in order (a parent
-- before its children), near the parent, at a steady angle; the roots on
-- a ring round the middle.
function M.place(s: Sim, saved: { [string]: any }?): number
	local placed = table.create(s.n, false)
	local fresh = 0
	for i = 1, s.n do
		if s.fixed[i] then
			s.x[i], s.y[i] = 0, 0
			placed[i] = true
			continue
		end
		local at = saved and saved[s.ids[i]]
		if type(at) == "table" and type(at[1]) == "number" and type(at[2]) == "number" then
			s.x[i], s.y[i] = at[1], at[2]
			placed[i] = true
		end
	end
	-- Each parent's children, in order: spread evenly round it, on a ring
	-- as wide as they need, facing away from its own parent (near where
	-- they'll rest, so a cold start settles in few steps).
	local kids: { [number]: { number } } = {}
	local roots: { number } = {}
	for i = 1, s.n do
		local p = s.parent[i]
		if p > 0 then
			kids[p] = kids[p] or {}
			table.insert(kids[p], i)
		elseif not s.fixed[i] then
			table.insert(roots, i)
		end
	end
	local function put(i: number)
		if placed[i] then
			return
		end
		local p = s.parent[i]
		if p > 0 then
			put(p)
			local sib = kids[p]
			local k = table.find(sib, i) or 1
			local gp = s.parent[p]
			local hub = gp == 0
			local len = if hub then M.HUB_LEN else M.STRONG_LEN
			local base, spread
			if hub and (s.fixed[p] or gp == 0) and s.x[p] == 0 and s.y[p] == 0 then
				base, spread = -math.pi / 2, math.pi * 2
			else
				base = math.atan2(s.y[p] - (if gp > 0 then s.y[gp] else 0), s.x[p] - (if gp > 0 then s.x[gp] else 0))
				spread = math.pi * 5 / 3
			end
			local r = if hub and spread == math.pi * 2 then len + M.ring(#(kids[i] or {}), M.STRONG_LEN) else M.ring(#sib, len)
			local a = if spread >= math.pi * 2 then base + (k - 1) / #sib * spread else base + ((k - 0.5) / #sib - 0.5) * spread
			-- (A little steady jitter: never two exactly in line.)
			a += (hash01(s.ids[i], s.seed) - 0.5) * 0.08
			s.x[i], s.y[i] = s.x[p] + math.cos(a) * r, s.y[p] + math.sin(a) * r
		else
			local k = table.find(roots, i) or 1
			local a = (k - 1) / math.max(1, #roots) * math.pi * 2
			local r = if #roots > 1 then M.HUB_LEN else 0
			s.x[i], s.y[i] = math.cos(a) * r, math.sin(a) * r
		end
		placed[i] = true
		s.fresh[i] = true
		fresh += 1
	end
	for i = 1, s.n do
		put(i)
	end
	for i = 1, s.n do
		s.vx[i], s.vy[i] = 0, 0
	end
	return fresh
end

-- Wake `ids` (indices) and their neighbours; nil wakes all.
function M.wake(s: Sim, ids: { number }?)
	if ids == nil then
		for i = 1, s.n do
			s.awake[i] = true
		end
	else
		for i = 1, s.n do
			s.awake[i] = false
		end
		for _, i in ids do
			s.awake[i] = true
			for _, j in s.nbrs[i] do
				s.awake[j] = true
			end
		end
	end
	s.energy = math.huge
end

function M.sleep(s: Sim)
	for i = 1, s.n do
		s.awake[i] = false
		s.vx[i], s.vy[i] = 0, 0
	end
	s.energy = 0
end

local function cell_key(cx: number, cy: number): number
	return (cx + 32768) * 65536 + (cy + 32768)
end

function M.step(s: Sim): number
	local n = s.n
	local x, y, vx, vy, awake, held, mass = s.x, s.y, s.vx, s.vy, s.awake, s.held, s.mass
	local R = M.REPEL_R
	-- The grid, built in place.
	local cells, nxt = s.cells, s.next_in
	table.clear(cells)
	for i = 1, n do
		local key = cell_key(math.floor(x[i] / R), math.floor(y[i] / R))
		nxt[i] = cells[key] or 0
		cells[key] = i
	end
	-- Springs.
	for _, l in s.links do
		local a, b = l.a, l.b
		if not (awake[a] or awake[b]) then
			continue
		end
		local ex, ey = x[b] - x[a], y[b] - y[a]
		local d = math.max(0.01, math.sqrt(ex * ex + ey * ey))
		local len = l.len or M.WEAK_LEN
		local k = if l.strong then M.STRONG_K else M.WEAK_K
		local f = (d - len) * k
		local ux, uy = ex / d * f, ey / d * f
		if awake[a] and not held[a] then
			vx[a] += ux / mass[a]
			vy[a] += uy / mass[a]
		end
		if awake[b] and not held[b] then
			vx[b] -= ux / mass[b]
			vy[b] -= uy / mass[b]
		end
	end
	-- Forces, into the velocities of those awake (with the springs', then
	-- damped).
	local R2 = R * R
	for i = 1, n do
		if not awake[i] or held[i] then
			continue
		end
		local fx, fy = -x[i] * M.CENTER, -y[i] * M.CENTER
		local cx, cy = math.floor(x[i] / R), math.floor(y[i] / R)
		for dx = -1, 1 do
			for dy = -1, 1 do
				local j = cells[cell_key(cx + dx, cy + dy)] or 0
				while j > 0 do
					if j ~= i then
						local ex, ey = x[i] - x[j], y[i] - y[j]
						local d2 = ex * ex + ey * ey
						if d2 < R2 then
							if d2 < 1 then
								-- (On top of each other: apart at a steady angle.)
								local a = hash01(s.ids[i], i + j) * math.pi * 2
								ex, ey, d2 = math.cos(a), math.sin(a), 1
							end
							local f = M.REPEL * mass[j] / d2 * (1 - d2 / R2)
							local d = math.sqrt(d2)
							fx += ex / d * f
							fy += ey / d * f
						end
					end
					j = nxt[j]
				end
			end
		end
		vx[i] = (vx[i] + fx / mass[i]) * M.DAMP
		vy[i] = (vy[i] + fy / mass[i]) * M.DAMP
	end
	-- Move.
	local e, awake_n = 0, 0
	for i = 1, n do
		if awake[i] and not held[i] then
			local v2 = vx[i] * vx[i] + vy[i] * vy[i]
			if v2 > M.MAX_V * M.MAX_V then
				local k = M.MAX_V / math.sqrt(v2)
				vx[i] *= k
				vy[i] *= k
				v2 = M.MAX_V * M.MAX_V
			end
			x[i] += vx[i]
			y[i] += vy[i]
			e += v2
			awake_n += 1
		end
	end
	s.steps += 1
	s.energy = if awake_n > 0 then e / awake_n else 0
	return s.energy
end

function M.still(s: Sim): boolean
	return s.energy <= M.REST
end

-- Steps until still, `budget_s` spent or MAX_STEPS since waking.
function M.run(s: Sim, budget_s: number, clock: () -> number): boolean
	local t0 = clock()
	repeat
		M.step(s)
		if M.still(s) then
			M.sleep(s)
			return true
		end
	until clock() - t0 >= budget_s
	return false
end

function M.positions(s: Sim): { [string]: { number } }
	local out = {}
	for i = 1, s.n do
		out[s.ids[i]] = { math.floor(s.x[i] * 10 + 0.5) / 10, math.floor(s.y[i] * 10 + 0.5) / 10 }
	end
	return out
end

-- The box of every node: x0, y0, x1, y1.
function M.bounds(s: Sim): (number, number, number, number)
	local x0, y0, x1, y1 = math.huge, math.huge, -math.huge, -math.huge
	for i = 1, s.n do
		local px, py = s.x[i] :: number, s.y[i] :: number
		x0, y0 = math.min(x0, px), math.min(y0, py)
		x1, y1 = math.max(x1, px), math.max(y1, py)
	end
	if x0 > x1 then
		return -1, -1, 1, 1
	end
	return x0, y0, x1, y1
end

return M

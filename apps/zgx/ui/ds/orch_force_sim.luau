--!native
--!optimize 2
-- The force Graph's physics (ds/orch_force), pure: no requires, no clock,
-- no randomness but its own seeded one. Like Obsidian's graph: strong
-- links are stiff, short springs; weak links soft, long ones; every node
-- pushes its near neighbours away (a uniform grid, so a step is O(n),
-- never n²). A tree radiates out from the middle: each node has a slot
-- round its parent (its ring and its angle), facing away from where the
-- parent's own link comes in, and a soft pull towards it; so a crew fans
-- out on the far side of its orchestrator, in rows, never wrapped round
-- it. A step moves only the nodes awake (all, on a cold start; what's new
-- and its neighbours after a change; one dragged and its neighbours).
--
--   sim.new(nodes, links, seed?)   Sim: nodes { id, parent?, mass?, fixed? },
--                                  links { a, b, strong, hub? } (indices)
--   sim.place(s, saved)            positions: kept ones ({ [id] = { x, y } })
--                                  where known, else in their slots; how
--                                  many were new (`s.fresh[i]`)
--   sim.wake(s, ids?)              wake these and their neighbours (all, nil)
--   sim.step(s)                    one step; its energy (mean v²)
--   sim.run(s, budget_s, clock)    steps until still or `budget_s` spent;
--                                  whether still
--   sim.still(s)                   at rest
--   sim.positions(s)               { [id] = { x, y } }, to keep

local M = {}

-- Strong links: stiff. A hub (an orchestrator to the Brain) further out
-- than a chat to its orchestrator.
M.STRONG_K = 0.22
M.HUB_LEN = 150
-- A crew's fan: its first row this far out, each next row FAN_ROW more, a
-- node every FAN_GAP along a row, the fan FAN_WIDE (radians) across.
M.FAN_LEN, M.FAN_ROW, M.FAN_GAP = 74, 38, 34
M.FAN_WIDE = math.rad(150)
-- The pull towards a node's slot.
M.SLOT_K = 0.05
-- The middle's own chats keep this far (radians) from a hub's way out.
M.HUB_CLEAR = math.rad(22)
-- Weak links: soft and long.
M.WEAK_LEN, M.WEAK_K = 240, 0.004
M.REPEL = 1500
M.REPEL_R = 70 -- past this, no push (the grid's cell)
M.DAMP = 0.55
M.MAX_V = 24
-- At rest: the mean v² of those awake below this (under a third of a
-- point a step: still to the eye).
M.REST = 0.1
M.MAX_STEPS = 400

-- Where the middle's k-th hub goes (radians, 0 to the right): first right,
-- then left (the field's widest way), then the slants between, then up
-- and down, then the golden angle on; the same for a hub whatever comes
-- after it, so a new one never moves those before.
local HUB_SEQ = { 0, 180, 315, 135, 45, 225, 340, 160, 20, 200, 270, 90 }
function M.hub_angle(k: number): number
	local deg = HUB_SEQ[k] or ((k - #HUB_SEQ) * 137.508) % 360
	return math.rad(deg)
end

-- (`fixed`: held where it is, the middle: the Brain.)
export type Node = { id: string, parent: number?, mass: number?, fixed: boolean? }
-- (`len`: its rest length; a strong link's is its child's ring.)
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
	-- Each node's slot: how far from its parent, at what angle (from the
	-- direction its parent faces: away from the grandparent; for the
	-- middle's children, from straight up).
	slot_r: { number },
	slot_a: { number },
	-- How deep a node's own fan reaches (its last row), for its hub link.
	depth: { number },
	links: { Link },
	nbrs: { { number } },
	seed: number,
	steps: number,
	energy: number,
	cells: { [number]: number },
	next_in: { number },
	-- Round the middle: across and down (the field's shape), and the
	-- spacing's scale.
	sx: number,
	sy: number,
	k: number,
}

-- A steady number in [0, 1) from a string and a seed.
local function hash01(s: string, seed: number): number
	local h = seed % 2147483647
	for i = 1, #s do
		h = (h * 31 + string.byte(s, i)) % 2147483647
	end
	h = (h * 1103515245 + 12345) % 2147483648
	return h / 2147483648
end
M.hash01 = hash01

-- How `count` children sit in rows on a fan: each one's row radius and
-- angle (across the fan, centred), and the last row's radius.
local function fan(count: number, scale: number?): ({ number }, { number }, number)
	local sk = scale or 1
	local len, rowd, gap = M.FAN_LEN * sk, M.FAN_ROW * sk, M.FAN_GAP * sk
	local rs, as = table.create(count, 0), table.create(count, 0)
	local k, row, last = 1, 0, len
	while k <= count do
		local r = len + row * rowd
		local cap = math.max(3, math.floor(r * M.FAN_WIDE / gap))
		local here = math.min(cap, count - k + 1)
		-- (A row holding fewer than it could: its nodes closer together,
		-- centred, never further apart than its gap.)
		local span = math.min(M.FAN_WIDE, (here - 1) * gap / r)
		-- (Every other row shifted half a place: no link runs behind a node
		-- of the row before.)
		local shift = if row % 2 == 1 and here > 1 then span / (here - 1) / 2 else 0
		for j = 0, here - 1 do
			rs[k] = r
			as[k] = (if here == 1 then 0 else -span / 2 + j * span / (here - 1)) + (if j == here - 1 then -shift else shift)
			k += 1
		end
		last = r
		row += 1
	end
	return rs, as, last
end
M.fan = fan

-- `opts.aspect`: the field's width over its height: round the middle, the
-- web stretched across a wide field (an ellipse), never wasted room up
-- and down. `opts.dense`: 1 for the full spacing, less for a crowded web.
function M.new(nodes: { Node }, links: { Link }, seed: number?, opts: { aspect: number?, dense: number? }?): Sim
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
		fresh = table.create(n, false),
		fixed = table.create(n, false),
		slot_r = table.create(n, 0),
		slot_a = table.create(n, 0),
		depth = table.create(n, 0),
		links = links,
		nbrs = table.create(n) :: any,
		seed = seed or 7,
		steps = 0,
		energy = math.huge,
		cells = {},
		next_in = table.create(n, 0),
		sx = 1,
		sy = 1,
		k = 1,
	}
	local aspect = math.clamp((opts and opts.aspect) or 1, 0.6, 3)
	s.sx, s.sy = math.sqrt(aspect), 1 / math.sqrt(aspect)
	s.k = math.clamp((opts and opts.dense) or 1, 0.7, 1)
	for i, nd in nodes do
		s.ids[i] = nd.id
		s.index[nd.id] = i
		s.parent[i] = nd.parent or 0
		s.mass[i] = nd.mass or 1
		s.nbrs[i] = {}
		s.fixed[i] = nd.fixed == true
		s.held[i] = nd.fixed == true
	end
	for _, l in links do
		table.insert(s.nbrs[l.a], l.b)
		table.insert(s.nbrs[l.b], l.a)
	end
	-- Slots: each parent's children in their order; the middle's spread
	-- all round it (hubs first, evenly), every other's on its fan.
	local kids: { [number]: { number } } = {}
	for i = 1, n do
		local p = s.parent[i]
		if p > 0 then
			kids[p] = kids[p] or {}
			table.insert(kids[p], i)
		end
	end
	for p, list in kids do
		if s.parent[p] == 0 then
			continue
		end
		local rs, as, last = fan(#list, s.k)
		for k, c in list do
			s.slot_r[c], s.slot_a[c] = rs[k], as[k]
		end
		s.depth[p] = last
	end
	for p, list in kids do
		if s.parent[p] ~= 0 then
			continue
		end
		-- (Round the middle: the hubs evenly all round it, each out past
		-- its own fan's reach; the middle's own chats on a ring inside,
		-- between them.)
		local hubs, own = {}, {}
		for _, c in list do
			table.insert(if kids[c] then hubs else own, c)
		end
		for k, c in hubs do
			s.slot_a[c] = M.hub_angle(k)
			s.slot_r[c] = math.max(M.HUB_LEN * s.k, s.depth[c] + 110 * s.k)
		end
		-- (The middle's own chats: rings round it, as many a ring as fit
		-- a gap apart, none within HUB_CLEAR of a hub's way out.)
		local hub_a: { number } = {}
		for _, c in hubs do
			table.insert(hub_a, s.slot_a[c])
		end
		local function clear_of_hubs(a: number): boolean
			for _, h in hub_a do
				local d = math.abs((a - h + math.pi) % (2 * math.pi) - math.pi)
				if d < M.HUB_CLEAR then
					return false
				end
			end
			return true
		end
		local k, ring = 1, 0
		while k <= #own do
			local r = (M.FAN_LEN * 1.15 + ring * M.FAN_ROW) * s.k
			local cap = math.max(3, math.floor(2 * math.pi * r / (M.FAN_GAP * s.k)))
			local spots: { number } = {}
			for j = 0, cap - 1 do
				local a = (j + (ring % 2) * 0.5) / cap * math.pi * 2 + math.pi / cap
				if clear_of_hubs(a) then
					table.insert(spots, a)
				end
			end
			-- (Spread over the spots this ring has, not bunched at its start.)
			local here = math.min(#spots, #own - k + 1)
			for j = 1, here do
				local spot = spots[math.max(1, math.floor((j - 0.5) * #spots / here + 0.5))]
				local c = own[k]
				s.slot_a[c] = spot
				s.slot_r[c] = r
				k += 1
			end
			ring += 1
		end
	end
	-- (A hub's mass grows with its crew: many stiff springs on one light
	-- node would ring, never rest.)
	for i = 1, n do
		local k = if kids[i] then #kids[i] else 0
		s.mass[i] = math.max(s.mass[i], 1 + k * M.STRONG_K * 1.5)
	end
	for _, l in links do
		l.len = if l.strong then s.slot_r[l.b] else M.WEAK_LEN
	end
	return s
end

-- The direction node `p` faces: away from its parent; the middle, up.
local function facing(s: Sim, p: number): number
	local g = s.parent[p]
	if g == 0 then
		return -math.pi / 2
	end
	return math.atan2(s.y[p] - s.y[g], s.x[p] - s.x[g])
end

-- Where node `i`'s slot is now: round the middle, from straight up;
-- round anything else, from the way it faces.
local function slot(s: Sim, i: number): (number, number)
	local p = s.parent[i]
	if s.parent[p] == 0 then
		local a = s.slot_a[i]
		return s.x[p] + math.cos(a) * s.slot_r[i] * s.sx, s.y[p] + math.sin(a) * s.slot_r[i] * s.sy
	end
	local a = s.slot_a[i] + facing(s, p)
	return s.x[p] + math.cos(a) * s.slot_r[i], s.y[p] + math.sin(a) * s.slot_r[i]
end

-- Node `i` takes where it is now as its slot (dropped there, or kept
-- there from before): nothing pulls it back where it was.
function M.adopt(s: Sim, i: number)
	local p = s.parent[i]
	if p == 0 then
		return
	end
	local dx, dy = s.x[i] - s.x[p], s.y[i] - s.y[p]
	if s.parent[p] == 0 then
		-- (Round the middle: on its ellipse.)
		local ex, ey = dx / s.sx, dy / s.sy
		s.slot_a[i] = math.atan2(ey, ex)
		s.slot_r[i] = math.max(1, math.sqrt(ex * ex + ey * ey))
	else
		s.slot_a[i] = math.atan2(dy, dx) - facing(s, p)
		s.slot_r[i] = math.max(1, math.sqrt(dx * dx + dy * dy))
	end
	for _, l in s.links do
		if l.strong and l.b == i then
			l.len = s.slot_r[i]
		end
	end
end

-- Where everything starts: kept places first; then each in its slot, a
-- parent before its children (a little steady jitter: none exactly in
-- line).
function M.place(s: Sim, saved: { [string]: any }?): number
	local placed = table.create(s.n, false)
	local fresh = 0
	for i = 1, s.n do
		s.fresh[i] = false
		if s.fixed[i] then
			-- (The middle: where it was dropped, if it was; else the centre.)
			local at = saved and saved[s.ids[i]]
			if type(at) == "table" and type(at[1]) == "number" and type(at[2]) == "number" then
				s.x[i], s.y[i] = at[1], at[2]
			else
				s.x[i], s.y[i] = 0, 0
			end
			placed[i] = true
			continue
		end
		local at = saved and saved[s.ids[i]]
		if type(at) == "table" and type(at[1]) == "number" and type(at[2]) == "number" then
			s.x[i], s.y[i] = at[1], at[2]
			placed[i] = true
		end
	end
	local function put(i: number)
		if placed[i] then
			return
		end
		local p = s.parent[i]
		if p > 0 then
			put(p)
			local x, y = slot(s, i)
			local j = (hash01(s.ids[i], s.seed) - 0.5) * 2
			s.x[i], s.y[i] = x + j, y - j
		else
			s.x[i], s.y[i] = 0, 0
		end
		placed[i] = true
		s.fresh[i] = true
		fresh += 1
	end
	for i = 1, s.n do
		put(i)
	end
	-- (A kept place is its slot now: a reheat round it never pulls it
	-- back to where a fresh web would put it.)
	for i = 1, s.n do
		if not s.fresh[i] and not s.fixed[i] then
			M.adopt(s, i)
		end
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
	-- Springs (strong and weak), into the velocities.
	for _, l in s.links do
		local a, b = l.a, l.b
		if not (awake[a] or awake[b]) then
			continue
		end
		local ex, ey = x[b] - x[a], y[b] - y[a]
		local d = math.max(0.01, math.sqrt(ex * ex + ey * ey))
		local f = (d - (l.len or M.WEAK_LEN)) * (if l.strong then M.STRONG_K else M.WEAK_K)
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
	-- Each towards its slot (the fan facing away from where its parent's
	-- link comes in).
	for i = 1, n do
		if awake[i] and not held[i] and s.parent[i] > 0 then
			local tx, ty = slot(s, i)
			vx[i] += (tx - x[i]) * M.SLOT_K
			vy[i] += (ty - y[i]) * M.SLOT_K
		end
	end
	-- The push apart, through the grid (built in place).
	local R = M.REPEL_R
	local cells, nxt = s.cells, s.next_in
	table.clear(cells)
	for i = 1, n do
		local key = cell_key(math.floor(x[i] / R), math.floor(y[i] / R))
		nxt[i] = cells[key] or 0
		cells[key] = i
	end
	local R2 = R * R
	for i = 1, n do
		if not awake[i] or held[i] then
			continue
		end
		local fx, fy = 0, 0
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

-- Steps until still or `budget_s` spent.
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

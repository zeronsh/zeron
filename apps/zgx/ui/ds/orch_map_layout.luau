-- Where the Map's Topology (ds/orch_map_topology) puts everything, for a
-- world and a width. Pure, so it can be worked out and checked apart from
-- drawing.
--
-- Every machine is a lane across the width, its name and load in a column
-- of their own at its left (`lane_w`); in the lane, its projects, and in
-- each the orchestrators and threads at work there, one row each, all
-- starting at one left edge (`content_x`). Between the two run the rails:
-- one for each orchestrator with crew, in its hue, from its own row to its
-- last crew row, wherever that is, across the lines between machines for
-- crew on another. From the rail a branch runs to each of its rows and
-- ends `GAP` short of the row, at its middle, whatever the width. Rails
-- that never overlap share a column; a branch that crosses another
-- orchestrator's rail leaves a gap in that rail. On a machine a rail
-- arrives at, its rows come first; on one it leaves, last.
--
--   layout.layout(world, width)  Layout (below)
--   layout.lines(rail)           its solid and dashed polylines

local model = require("@ds/orch_map_model")

local M = {}

-- Geometry (points).
M.LANE_TOP = 10
M.LANE_MIN = 76
M.PROJECT_H = 24
M.ROW_H = 30
M.NODE_H = 32
M.LANE_PAD = 10
M.RAIL_PAD = 10
M.RAIL_STEP = 14
M.CORNER = 7
M.GAP = 3
-- How far a branch runs past the rightmost rail, before its gap.
M.REACH = 16

local LANE_TOP, LANE_MIN, PROJECT_H, ROW_H, NODE_H, LANE_PAD = M.LANE_TOP, M.LANE_MIN, M.PROJECT_H, M.ROW_H, M.NODE_H, M.LANE_PAD
local RAIL_PAD, RAIL_STEP, CORNER, GAP, REACH = M.RAIL_PAD, M.RAIL_STEP, M.CORNER, M.GAP, M.REACH

export type Lane = { id: string, name: string, y: number, h: number, online: boolean, here: boolean, platform: string?, cloud: boolean, device: any, count: number }
export type Label = { id: string, lane: string, name: string, y: number }
-- A row: an orchestrator's (`kind` "orchestrator"), a crew thread's or a
-- thread of the person's own. `mid` its middle's y.
export type Row = { id: string, lane: string, kind: string, x: number, y: number, w: number, h: number, mid: number, orchestrator: string?, hue: number?, paused: boolean }
export type Branch = { y: number, to: number, node: boolean }
-- A rail's way: its column's x, from `top` to `bottom`, with `gaps` (y
-- ranges left out where another's branch crosses) and `dashed` (y ranges
-- across another machine's head).
export type Rail = {
	orchestrator: string,
	hue: number?,
	paused: boolean,
	x: number,
	top: number,
	bottom: number,
	branches: { Branch },
	gaps: { { number } },
	dashed: { { number } },
}
export type Layout = {
	h: number,
	width: number,
	-- The machines' column, and where the rows start.
	lane_w: number,
	content_x: number,
	lanes: { Lane },
	labels: { Label },
	rows: { Row },
	rails: { Rail },
	-- Where a pulse starts or ends for a chat id: { x, y }.
	anchors: { [string]: { number } },
	-- The way from a crew row to its orchestrator's row, as points (x, y, …).
	routes: { [string]: { number } },
}

function M.layout(world: any, width: number): Layout
	local fleet = model.fleet(world)
	local lane_w = math.clamp(math.floor(width * 0.13), 100, 150)
	local out: Layout = { h = 0, width = width, lane_w = lane_w, content_x = 0, lanes = {}, labels = {}, rows = {}, rails = {}, anchors = {}, routes = {} }

	-- Which machine each orchestrator lives on, and the last its crew are on.
	local lane_index: { [string]: number } = {}
	for i, m in fleet.machines do
		lane_index[m.id] = i
	end
	local home: { [string]: number } = {}
	local first: { [string]: number } = {}
	local last: { [string]: number } = {}
	local crew_count: { [string]: number } = {}
	for _, t in fleet.threads do
		local li = lane_index[t.device or ""]
		if li and t.orchestrator then
			if t.kind == "orchestrator" then
				home[t.orchestrator] = li
			else
				crew_count[t.orchestrator] = (crew_count[t.orchestrator] or 0) + 1
			end
			first[t.orchestrator] = math.min(first[t.orchestrator] or li, li)
			last[t.orchestrator] = math.max(last[t.orchestrator] or li, li)
		end
	end
	-- -1: its rail arrives here from above; 1: it leaves for below.
	local function block_score(t: any, li: number): number
		local o = t.orchestrator
		if not o or not crew_count[o] then
			return 0
		end
		if (first[o] or li) < li then
			return -1
		end
		if (last[o] or li) > li then
			return 1
		end
		return 0
	end

	local y = 0
	local by_orch: { [string]: { Row } } = {}
	for li, m in fleet.machines do
		local lane: Lane = { id = m.id, name = m.name, y = y, h = 0, online = m.online, here = m.here, platform = m.platform, cloud = m.cloud, device = m.device, count = m.count }
		local top = y
		y += LANE_TOP
		local groups = table.clone(m.groups)
		local score: { [string]: number } = {}
		for _, g in groups do
			local lo, hi = 0, 0
			for _, t in g.threads do
				local s = block_score(t, li)
				lo, hi = math.min(lo, s), math.max(hi, s)
			end
			score[g.key] = if lo < 0 then -1 else hi
		end
		table.sort(groups, function(a: any, b: any): boolean
			if score[a.key] ~= score[b.key] then
				return score[a.key] < score[b.key]
			end
			-- (None last.)
			if (a.key == "") ~= (b.key == "") then
				return b.key == ""
			end
			return a.key < b.key
		end)
		for _, g in groups do
			if g.project then
				table.insert(out.labels, { id = `{m.id}/{g.key}`, lane = m.id, name = g.project :: string, y = y })
				y += PROJECT_H
			end
			local list = table.clone(g.threads)
			local pos: { [string]: number } = {}
			for i, t in list do
				pos[t.id] = i
			end
			table.sort(list, function(a: any, b: any): boolean
				local sa, sb = block_score(a, li), block_score(b, li)
				if sa ~= sb then
					return sa < sb
				end
				return pos[a.id] < pos[b.id]
			end)
			for _, t in list do
				local h = if t.kind == "orchestrator" then NODE_H else ROW_H
				local row: Row = { id = t.id, lane = m.id, kind = t.kind, x = 0, y = y, w = 0, h = h, mid = y + h / 2, orchestrator = t.orchestrator, hue = t.hue, paused = t.paused }
				table.insert(out.rows, row)
				if t.orchestrator then
					by_orch[t.orchestrator] = by_orch[t.orchestrator] or {}
					table.insert(by_orch[t.orchestrator], row)
				end
				y += h
			end
		end
		y += LANE_PAD
		y = math.max(y, top + LANE_MIN)
		lane.h = y - top
		table.insert(out.lanes, lane)
	end
	out.h = y

	-- The rails: each orchestrator with crew shown, in the first free
	-- column (rails that never overlap share one).
	local spans = {}
	for o, rows in by_orch do
		local has_crew = false
		local top_, bottom = math.huge, -math.huge
		for _, r in rows do
			has_crew = has_crew or r.kind ~= "orchestrator"
			top_ = math.min(top_, r.mid)
			bottom = math.max(bottom, r.mid)
		end
		if has_crew then
			table.insert(spans, { o = o, top = top_, bottom = bottom, rows = rows, col = 0 })
		end
	end
	table.sort(spans, function(a: any, b: any): boolean
		if a.top ~= b.top then
			return a.top < b.top
		end
		return a.o < b.o
	end)
	local columns: { number } = {}
	for _, s in spans do
		local col = 0
		for c, bottom in columns do
			if bottom < s.top - CORNER * 2 then
				col = c
				break
			end
		end
		if col == 0 then
			table.insert(columns, 0)
			col = #columns
		end
		columns[col] = s.bottom
		s.col = col
	end
	local ncols = #columns
	local rails_x = lane_w + RAIL_PAD
	local content_x = if ncols > 0 then rails_x + (ncols - 1) * RAIL_STEP + REACH + GAP else lane_w + RAIL_PAD
	out.content_x = content_x
	for _, r in out.rows do
		r.x = content_x
		r.w = math.max(0, width - content_x)
		out.anchors[r.id] = { content_x - GAP, r.mid }
	end

	for _, s in spans do
		local branches: { Branch } = {}
		for _, r in s.rows do
			table.insert(branches, { y = r.mid, to = content_x - GAP, node = r.kind == "orchestrator" })
		end
		table.sort(branches, function(a: Branch, b: Branch): boolean
			return a.y < b.y
		end)
		local o = fleet.orchestrators[s.o] or {}
		table.insert(out.rails, {
			orchestrator = s.o,
			hue = o.hue,
			paused = o.paused == true,
			x = rails_x + (s.col - 1) * RAIL_STEP,
			top = s.top,
			bottom = s.bottom,
			branches = branches,
			gaps = {},
			dashed = {},
		})
	end

	-- Where another's branch crosses a rail, the rail breaks.
	for _, rail in out.rails do
		for _, other in out.rails do
			if other ~= rail and other.x < rail.x then
				for _, b in other.branches do
					if b.y > rail.top and b.y < rail.bottom then
						table.insert(rail.gaps, { b.y - 5, b.y + 5 })
					end
				end
			end
		end
	end

	-- Each crew row's way back to its orchestrator's row, for a pulse.
	for _, rail in out.rails do
		local node_y: number? = nil
		for _, b in rail.branches do
			if b.node then
				node_y = b.y
			end
		end
		for _, r in by_orch[rail.orchestrator] or {} do
			if r.kind ~= "orchestrator" then
				local way = { content_x - GAP, r.mid, rail.x, r.mid }
				if node_y then
					table.insert(way, rail.x)
					table.insert(way, node_y :: number)
					table.insert(way, content_x - GAP)
					table.insert(way, node_y :: number)
				end
				out.routes[r.id] = way
			end
		end
	end
	return out
end

-- A rail's lines as polylines (x, y, …): the rail in its solid runs (gaps
-- and dashed runs left out) and each branch from the rail to its end, the
-- first and last rounding off the rail's ends (their corner point in the
-- middle: drawn as a curve); and the dashed runs.
function M.lines(rail: Rail): ({ { number } }, { { number } })
	local solid: { { number } } = {}
	local dashed: { { number } } = {}
	local n = #rail.branches
	if n == 1 then
		local b = rail.branches[1]
		table.insert(solid, { rail.x, b.y, b.to, b.y })
		return solid, dashed
	end
	local cuts = {}
	for _, g in rail.gaps do
		table.insert(cuts, { g[1], g[2], false })
	end
	for _, d in rail.dashed do
		table.insert(cuts, { d[1], d[2], true })
	end
	table.sort(cuts, function(a: any, b: any): boolean
		return a[1] < b[1]
	end)
	local at = rail.top + CORNER
	local stop = rail.bottom - CORNER
	for _, c in cuts do
		local a, b = math.max(c[1], at), math.min(c[2], stop)
		if b > a then
			if a > at then
				table.insert(solid, { rail.x, at, rail.x, a })
			end
			if c[3] then
				table.insert(dashed, { rail.x, a, rail.x, b })
			end
			at = b
		end
	end
	if stop > at then
		table.insert(solid, { rail.x, at, rail.x, stop })
	end
	for i, b in rail.branches do
		if i == 1 or i == n then
			local dy = if i == 1 then CORNER else -CORNER
			table.insert(solid, { rail.x, b.y + dy, rail.x, b.y, rail.x + CORNER, b.y, b.to, b.y })
		else
			table.insert(solid, { rail.x, b.y, b.to, b.y })
		end
	end
	return solid, dashed
end

return M

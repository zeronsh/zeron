-- Where the Map's Topology (ds/orch_map_topology) puts everything, for a
-- world and a width. Pure, so it can be worked out and checked apart from
-- drawing.
--
-- Every machine is a lane across the width, its head (ds/orch_machine_head,
-- one line) across its top; under it, its projects, and in each the
-- orchestrators and threads at work there, one row each, an orchestrator's
-- dot under the machine's name. An
-- orchestrator leads its crew: its rail drops from under its own dot, and
-- its crew sit indented to the right of it, an elbow from the rail into
-- each, ending `GAP` short of the row at its middle, whatever the width.
-- Crew on another machine: the rail runs on down across the line between
-- machines and elbows into them there, still indented from it. Where a
-- rail would run past rows not its own (another orchestrator's, on its
-- way to a machine further down), it steps left into a channel of its own
-- beside the rows and back in after them: every orchestrator keeps the
-- same left edge as its peers, and nothing sits on a rail. On a machine a
-- rail arrives at, its rows come first; on one it leaves, last.
--
--   layout.layout(world, width)  Layout (below)
--   layout.lines(rail)           its polylines

local model = require("@ds/orch_map_model")

local M = {}

-- Geometry (points).
-- A machine's head: its band (the head's line, room above and below).
M.HEAD_H = 40
M.HEAD_Y = 6
M.PROJECT_H = 24
M.ROW_H = 30
M.NODE_H = 32
M.LANE_PAD = 10
-- Where the rows start: an orchestrator's dot under the machine's name
-- (its head's mark, 14, and a gap, 8).
M.INSET = 22 - 9
-- An orchestrator's dot: its centre from its row's left edge, its radius.
M.DOT = 9
M.DOT_R = 5
-- How far right of its rail a crew row starts.
M.INDENT = 22
-- A channel's width, left of the rows, for each rail running past rows
-- not its own at once.
M.CHANNEL = 10
M.CORNER = 7
M.GAP = 3

local HEAD_H, PROJECT_H, ROW_H, NODE_H, LANE_PAD = M.HEAD_H, M.PROJECT_H, M.ROW_H, M.NODE_H, M.LANE_PAD
local INSET, DOT, DOT_R, INDENT, CHANNEL, CORNER, GAP = M.INSET, M.DOT, M.DOT_R, M.INDENT, M.CHANNEL, M.CORNER, M.GAP

-- `idle_x`: where a machine with nothing at work says so (right of any
-- rail running down past it, like a project's name).
export type Lane = { id: string, name: string, y: number, h: number, online: boolean, here: boolean, platform: string?, cloud: boolean, device: any, count: number, idle_x: number }
export type Label = { id: string, lane: string, name: string, y: number, x: number }
-- A row: an orchestrator's (`kind` "orchestrator"), a crew thread's or a
-- thread of the person's own. `mid` its middle's y.
export type Row = { id: string, lane: string, kind: string, x: number, y: number, w: number, h: number, mid: number, orchestrator: string?, hue: number?, paused: boolean, origin: string? }
-- An elbow into a crew row: the row's middle, and where it ends.
export type Branch = { y: number, to: number }
-- An orchestrator's rail: down (or up) from its dot at (`x`, `dot`), from
-- `top` to `bottom` (its rows' middles), drawn as `runs` (polylines, from
-- row to row: straight, or by its channel past rows not its own).
export type Rail = {
	orchestrator: string,
	hue: number?,
	paused: boolean,
	x: number,
	dot: number,
	top: number,
	bottom: number,
	branches: { Branch },
	runs: { { number } },
	-- The Brain dump's: what it started, not crew it leads (drawn lighter,
	-- dashed).
	dashed: boolean,
	-- ("tags") an edge grouping an orchestrator's rows on another machine.
	edge: boolean?,
}
export type Layout = {
	h: number,
	width: number,
	-- Where the rows start (the orchestrators' left edge).
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

-- Polylines with their straight upright pieces cut where they cross a
-- band (y ranges): what's left, as polylines. (Corners never sit in one.)
local function cut(lines: { { number } }, bands: { { number } }, only_x: number?): { { number } }
	local out: { { number } } = {}
	for _, pts in lines do
		local cur = { pts[1], pts[2] }
		for i = 3, #pts - 1, 2 do
			local x0, y0, x1, y1 = pts[i - 2], pts[i - 1], pts[i], pts[i + 1]
			if x0 == x1 and y0 ~= y1 and (only_x == nil or math.abs(x0 - only_x) < 0.5) then
				local dir = if y1 > y0 then 1 else -1
				local hits = {}
				for _, b in bands do
					local a, z = math.max(b[1], math.min(y0, y1)), math.min(b[2], math.max(y0, y1))
					if z > a then
						table.insert(hits, if dir > 0 then { a, z } else { z, a })
					end
				end
				table.sort(hits, function(p: { number }, q: { number }): boolean
					return (p[1] - q[1]) * dir < 0
				end)
				for _, h in hits do
					table.insert(cur, x0)
					table.insert(cur, h[1])
					if #cur >= 4 then
						table.insert(out, cur)
					end
					cur = { x0, h[2] }
				end
			end
			table.insert(cur, x1)
			table.insert(cur, y1)
		end
		if #cur >= 4 then
			table.insert(out, cur)
		end
	end
	return out
end

-- How an orchestrator's work on other machines is shown (ds/orch_map_
-- topo_style):
--   "lines"  its rail runs down a column of its own to them (the default)
--   "tags"   no line across machines: its rows there grouped by an edge in
--            its hue, each naming it
function M.layout(world: any, width: number, style: string?): Layout
	style = if style == "tags" then "edge" else "columns"
	local fleet = model.fleet(world)
	local out: Layout = { h = 0, width = width, content_x = 0, lanes = {}, labels = {}, rows = {}, rails = {}, anchors = {}, routes = {} }

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
	local away: { [string]: { Row } } = {}
	for li, m in fleet.machines do
		local lane: Lane = { id = m.id, name = m.name, y = y, h = 0, online = m.online, here = m.here, platform = m.platform, cloud = m.cloud, device = m.device, count = m.count, idle_x = 0 }
		local top = y
		y += HEAD_H
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
				table.insert(out.labels, { id = `{m.id}/{g.key}`, lane = m.id, name = g.project :: string, y = y, x = 0 })
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
					-- (Kept apart, in "tag" and "edge", when on a machine other
					-- than its orchestrator's: no line reaches it there.)
					local hl = home[t.orchestrator]
					if style == "edge" and t.kind ~= "orchestrator" and hl and hl ~= li then
						row.origin = t.orchestrator
						away[t.orchestrator] = away[t.orchestrator] or {}
						table.insert(away[t.orchestrator], row)
					else
						by_orch[t.orchestrator] = by_orch[t.orchestrator] or {}
						table.insert(by_orch[t.orchestrator], row)
					end
				end
				y += h
			end
		end
		if m.count == 0 then
			y += ROW_H
		end
		y += LANE_PAD
		lane.h = y - top
		table.insert(out.lanes, lane)
	end
	out.h = y

	-- Each orchestrator's rail: down (or up) from its dot through its crew
	-- rows. Where rows not its own lie between two of its own (another
	-- orchestrator's, on the way to another machine), it steps left into a
	-- channel of its own beside the rows, runs past them there and steps
	-- back in: every orchestrator keeps the same left edge as its peers.
	local channels = 0
	type Own = { mid: number, top: number, bottom: number, dot: boolean }
	-- `column`: a rail across machines keeps a gutter column of its own,
	-- the whole way (0 till given); `hop` its step from its dot into it.
	type Span = { o: string, x: number, dot: number, top: number, bottom: number, own: { Own }, crew: { Row }, passes: { { number } }, node: Row?, column: number?, hop: { number }? }
	local spans: { Span } = {}
	local span_of: { [string]: Span } = {}
	for o, rows in by_orch do
		local node: Row? = nil
		local crew = {}
		for _, r in rows do
			if r.kind == "orchestrator" then
				node = r
			else
				table.insert(crew, r)
			end
		end
		if #crew > 0 then
			local own: { Own } = {}
			for _, r in rows do
				table.insert(own, { mid = r.mid, top = r.y, bottom = r.y + r.h, dot = r == node })
			end
			table.sort(own, function(a: Own, b: Own): boolean
				return a.mid < b.mid
			end)
			-- (No row of its own shown: the rail starts at its first crew row.)
			local dot = if node then node.mid else crew[1].mid
			local lanes_of: { [string]: boolean } = {}
			local n_lanes = 0
			for _, r in rows do
				if not lanes_of[r.lane] then
					lanes_of[r.lane] = true
					n_lanes += 1
				end
			end
			local sp: Span = { o = o, x = 0, dot = dot, top = own[1].mid, bottom = own[#own].mid, own = own, crew = crew, passes = {}, node = node, column = if n_lanes > 1 then 0 else nil }
			-- Between two of its rows with another's between them: a pass
			-- (a rail with a column of its own needs none).
			for i = 1, if sp.column then 0 else #own - 1 do
				local a, b = own[i], own[i + 1]
				for _, r in out.rows do
					if r.orchestrator ~= o and r.mid > a.mid and r.mid < b.mid then
						table.insert(sp.passes, { a.bottom, b.top, 0 })
						break
					end
				end
			end
			table.insert(spans, sp)
			span_of[o] = sp
		end
	end
	-- The channels: passes that overlap take different ones.
	local all = {}
	for _, sp in spans do
		for _, p in sp.passes do
			table.insert(all, p)
		end
	end
	table.sort(all, function(a: { number }, b: { number }): boolean
		return a[1] < b[1]
	end)
	local ends: { number } = {}
	for _, p in all do
		local k = 0
		for i, e in ends do
			if e < p[1] - CORNER then
				k = i
				break
			end
		end
		if k == 0 then
			table.insert(ends, 0)
			k = #ends
		end
		ends[k] = p[2]
		p[3] = k
	end
	-- The rails across machines first, leftmost the one starting highest:
	-- each a column of its own; then the passes' channels after them.
	local across: { Span } = {}
	for _, sp in spans do
		if sp.column then
			table.insert(across, sp)
		end
	end
	table.sort(across, function(a: Span, b: Span): boolean
		if a.dot ~= b.dot then
			return a.dot < b.dot
		end
		return a.o < b.o
	end)
	for i, sp in across do
		sp.column = i
	end
	for _, p in all do
		p[3] += #across
	end
	channels = #ends + #across
	-- (The channels, then a little room before the rows.)
	local base = INSET + channels * CHANNEL
	out.content_x = base
	for _, r in out.rows do
		r.x = if r.origin then base + DOT + INDENT
			elseif r.kind == "crew" and r.orchestrator and span_of[r.orchestrator] then base + DOT + INDENT
			else base
		r.w = math.max(0, width - r.x)
		out.anchors[r.id] = if r.kind == "orchestrator" then { r.x + DOT, r.mid } else { r.x - GAP, r.mid }
	end

	local clear = DOT_R + 3
	local bands: { { number } } = {}
	for _, lane in out.lanes do
		if lane.y > 0 then
			table.insert(bands, { lane.y + 2, lane.y + HEAD_H - 4 })
		end
	end
	for _, sp in spans do
		local x = base + DOT
		sp.x = x
		-- Its runs: from each of its rows to the next, straight or by its
		-- channel; clear of its dot.
		local paths: { { number } } = {}
		local pass_at: { [number]: { number } } = {}
		for _, p in sp.passes do
			pass_at[p[1]] = p
		end
		local head, tail = sp.own[1], sp.own[#sp.own]
		local col = sp.column
		if col then
			-- Its column, the whole way; from its dot a hop left into it.
			local cx = INSET + (channels - col) * CHANNEL + CHANNEL / 2 - 4
			x = cx
			sp.x = cx
			local lo, hi = head.mid + CORNER, tail.mid - CORNER
			if sp.node then
				local node = sp.node :: Row
				local yh = node.y + node.h - 3
				local dx = base + DOT
				table.insert(paths, { dx, node.mid + clear, dx, yh - CORNER, dx, yh, dx - CORNER, yh, cx + CORNER, yh, cx, yh, cx, yh + CORNER })
				sp.hop = { yh, cx, dx }
				-- (Down from the hop, and up too if crew sit above it.)
				if hi > yh + CORNER then
					table.insert(paths, { cx, yh + CORNER, cx, hi })
				end
				if head.mid < node.mid then
					table.insert(paths, { cx, yh - 0.01, cx, head.mid + CORNER })
				end
			elseif hi > lo then
				table.insert(paths, { cx, lo, cx, hi })
			end
		end
		for i = 1, if col then 0 else #sp.own - 1 do
			local a, b = sp.own[i], sp.own[i + 1]
			local y1 = if a.dot then a.mid + clear else a.mid
			local y2 = if b.dot then b.mid - clear else b.mid
			-- (The rail's ends round off into their elbows.)
			if a == head and not a.dot then
				y1 = a.mid + CORNER
			end
			if b == tail and not b.dot then
				y2 = b.mid - CORNER
			end
			local p = pass_at[a.bottom]
			if p then
				local ch = INSET + (channels - p[3]) * CHANNEL + CHANNEL / 2 - 4
				local ya, yb = p[1], p[2]
				table.insert(paths, {
					x, y1, x, ya - CORNER, x, ya, x - CORNER, ya,
					ch + CORNER, ya, ch, ya, ch, ya + CORNER,
					ch, yb - CORNER, ch, yb, ch + CORNER, yb,
					x - CORNER, yb, x, yb, x, yb + CORNER, x, y2,
				})
			elseif y2 > y1 then
				table.insert(paths, { x, y1, x, y2 })
			end
		end
		-- (Past another machine's head it goes under it: a break in the line.)
		paths = cut(paths, bands)
		local branches: { Branch } = {}
		for _, r in sp.crew do
			table.insert(branches, { y = r.mid, to = r.x - GAP })
		end
		table.sort(branches, function(a: Branch, b: Branch): boolean
			return a.y < b.y
		end)
		local o = fleet.orchestrators[sp.o] or {}
		table.insert(out.rails, {
			orchestrator = sp.o,
			hue = o.hue,
			paused = o.paused == true,
			x = x,
			dot = sp.dot,
			top = sp.top,
			bottom = sp.bottom,
			branches = branches,
			runs = paths,
			dashed = o.kind == "brain_dump",
		})
	end
	-- "edge": each orchestrator's rows on another machine grouped by an edge
	-- in its hue, left of them, no branches.
	if style == "edge" then
		for o, rows in away do
			local by_lane: { [string]: { Row } } = {}
			for _, r in rows do
				by_lane[r.lane] = by_lane[r.lane] or {}
				table.insert(by_lane[r.lane], r)
			end
			local oo = fleet.orchestrators[o] or {}
			for _, list in by_lane do
				table.sort(list, function(a: Row, b: Row): boolean
					return a.y < b.y
				end)
				local x = base + DOT + 6
				local y0, y1 = list[1].y + 5, list[#list].y + list[#list].h - 5
				table.insert(out.rails, {
					orchestrator = o,
					hue = oo.hue,
					paused = oo.paused == true,
					x = x,
					dot = y0,
					top = y0,
					bottom = y1,
					branches = {},
					runs = { { x, y0, x, y1 } },
					dashed = oo.kind == "brain_dump",
					edge = true,
				})
			end
		end
	end

	-- Where an elbow (or a rail's hop from its dot) passes over another
	-- rail's column, that rail breaks round it: a hop, never a crossing.
	for _, a in out.rails do
		local overs: { { number } } = {}
		for _, b in a.branches do
			table.insert(overs, { b.y, a.x, b.to })
		end
		local sp = span_of[a.orchestrator]
		if sp and sp.hop then
			table.insert(overs, sp.hop :: { number })
		end
		for _, o in overs do
			for _, other in out.rails do
				if other ~= a and other.x > math.min(o[2], o[3]) + 0.5 and other.x < math.max(o[2], o[3]) - 0.5 then
					other.runs = cut(other.runs, { { o[1] - 4, o[1] + 4 } }, other.x)
				end
			end
		end
	end

	-- A project's name, and a machine's word that nothing runs there: where
	-- its rows start, or with the crew where a rail runs on past it.
	local function past_rail(mid: number): boolean
		for _, rail in out.rails do
			for _, run in rail.runs do
				for i = 1, #run - 3, 2 do
					if run[i] == rail.x and run[i + 2] == rail.x and mid > math.min(run[i + 1], run[i + 3]) and mid < math.max(run[i + 1], run[i + 3]) then
						return true
					end
				end
			end
		end
		return false
	end
	for _, label in out.labels do
		label.x = if past_rail(label.y + PROJECT_H / 2) then base + DOT + INDENT - 4 else base
	end
	for _, lane in out.lanes do
		lane.idle_x = if lane.count == 0 and past_rail(lane.y + HEAD_H + ROW_H / 2) then base + DOT + INDENT - 4 else base + 4
	end

	-- Each crew row's way back to its orchestrator's dot, for a pulse: up
	-- its rail as drawn.
	for _, rail in out.rails do
		if rail.edge or not span_of[rail.orchestrator] then
			continue
		end
		for _, r in span_of[rail.orchestrator].crew do
			local way = { r.x - GAP, r.mid, rail.x, r.mid }
			-- (The runs between it and the dot, walked toward the dot.)
			local up = r.mid > rail.dot
			local list = table.clone(rail.runs)
			table.sort(list, function(a: { number }, b: { number }): boolean
				return if up then a[2] > b[2] else a[2] < b[2]
			end)
			for _, run in list do
				local lo, hi = math.min(run[2], run[#run]), math.max(run[2], run[#run])
				local between = if up then hi <= r.mid + 0.5 and lo >= rail.dot - 0.5 else lo >= r.mid - 0.5 and hi <= rail.dot + 0.5
				if between then
					local pts = if up then {} else run
					if up then
						for i = #run - 1, 1, -2 do
							table.insert(pts, run[i])
							table.insert(pts, run[i + 1])
						end
					end
					for _, v in pts do
						table.insert(way, v)
					end
				end
			end
			table.insert(way, rail.x)
			table.insert(way, rail.dot)
			out.routes[r.id] = way
		end
	end
	return out
end

-- A rail's lines as polylines (x, y, …): its runs (from row to row, clear
-- of its dot, by its channel past rows not its own), and an elbow into
-- each crew row: the first and last round off the rail's ends (a corner
-- point between two close ones, drawn as a curve), the others leave it
-- straight.
function M.lines(rail: Rail): { { number } }
	local out: { { number } } = table.clone(rail.runs)
	for _, b in rail.branches do
		if b.y == rail.bottom and b.y > rail.dot then
			table.insert(out, { rail.x, b.y - CORNER, rail.x, b.y, rail.x + CORNER, b.y, b.to, b.y })
		elseif b.y == rail.top and b.y < rail.dot then
			table.insert(out, { rail.x, b.y + CORNER, rail.x, b.y, rail.x + CORNER, b.y, b.to, b.y })
		else
			table.insert(out, { rail.x, b.y, b.to, b.y })
		end
	end
	return out
end

return M

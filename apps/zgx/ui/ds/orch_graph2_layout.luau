-- The Map's new Graph (ds/orch_graph2), laid out: where every machine,
-- orchestrator and thread sits, worked out once for what's on the Map and
-- the width it has, never again for a pan, a zoom or a frame. Pure (no
-- requires, no clock): the same scene and width give the same layout.
--
-- Machines share the field as a squarified treemap, each by how much it
-- runs; in a machine, its groups (an orchestrator and the crew it runs
-- there, the person's own threads) share its body the same way; in a
-- group, its threads sit in a grid of cells, the lead's line coming down
-- a trunk at the group's left, along a rail over each row and down a
-- short tick to each thread (a comb: every line orthogonal, none across a
-- name). Crew on another machine than its lead gather under a ghost of the
-- lead there, one straight line joining the two.
--
--   layout.layout(scene, w)   Layout (below)
--   layout.cell_at(l, x, y)   the node index at a point of the layout's
--                             own space (nil for none): grid arithmetic,
--                             no search
--   layout.path(l, i)         the polyline (flat x, y, …) from node i's
--                             lead to it, for a pulse
--
-- A scene: { machines = { { id, name, groups = { Group } } },
--            links = { { a, b, kind, hue } } }    (extra lines: "routed")
-- A group: { id, lead = Thread?, ghost = boolean?, members = { Thread } }
-- A thread: { id, title, kind, hue }   (kind: "orchestrator" | "brain" |
--                                       "crew" | "thread")

local M = {}

M.PAD = 12 -- round the field
M.GAP = 10 -- between machines
M.HEAD = 34 -- a machine's heading band
M.GROUP_GAP = 8 -- between groups in a machine
M.LEAD_H = 26 -- a group's lead row
M.TRUNK = 9 -- the trunk's inset from a group's left
M.CELL_X = 20 -- where cells start, from a group's left
M.CELL_H_MIN, M.CELL_H_MAX = 20, 34
-- (A row grows to this with room to spare, its cells' marks and names
-- staying their size.)
M.CELL_H_GROW = 44
M.CELL_W_IDEAL = 170
M.CELL_AREA = 170 * 30 -- a thread's share of the field, for its height
M.MIN_H, M.MAX_H = 380, 860

export type Rect = { x: number, y: number, w: number, h: number }
export type Node = {
	id: string,
	title: string,
	kind: string,
	hue: number?,
	machine: number,
	group: number,
	-- (The dot's middle; the cell's box for its name.)
	x: number,
	y: number,
	cx: number,
	cy: number,
	cw: number,
	ch: number,
	lead: boolean,
	ghost: boolean,
}
export type Group = {
	id: string,
	machine: number,
	rect: Rect,
	lead: number?, -- node index of its lead (or ghost)
	first: number, -- first member's node index (members are contiguous)
	count: number,
	cols: number,
	rows: number,
	cell_w: number,
	cell_h: number,
	gx: number, -- the grid's origin
	gy: number,
	trunk_x: number,
	trunk_y0: number,
	trunk_y1: number,
	hue: number?,
	-- Each row's rail: y, and the x it runs to (flat: y1, x1, y2, x2, …).
	rails: { number },
}
export type Machine = { id: string, name: string, rect: Rect, first_group: number, groups: number, count: number }
-- (`pts`: its route, flat x, y, …: up out of its group, along the gaps
-- between groups and machines, down into the other.)
export type Line = { a: number, b: number, kind: string, hue: number?, pts: { number } }
export type Layout = {
	w: number,
	h: number,
	machines: { Machine },
	groups: { Group },
	nodes: { Node },
	index: { [string]: number },
	-- Lines that aren't a comb's: a lead to its ghost, what the Brain dump
	-- handed a part to.
	lines: { Line },
	sig: string,
}

-- Squarified treemap (Bruls, Huizing, van Wijk) of `weights` (all > 0, in
-- the order given; kept, so it's stable) into `r`: a rect each.
local function worst(row: { number }, sum: number, side: number, scale: number): number
	local mx, mn = 0, math.huge
	for _, v in row do
		mx = math.max(mx, v)
		mn = math.min(mn, v)
	end
	local s2 = (sum * scale) ^ 2
	local side2 = side * side
	return math.max(side2 * mx * scale / s2, s2 / (side2 * mn * scale))
end

function M.treemap(weights: { number }, r: Rect): { Rect }
	local out: { Rect } = {}
	local total = 0
	for _, v in weights do
		total += v
	end
	if total <= 0 or #weights == 0 then
		return out
	end
	local x, y, w, h = r.x, r.y, r.w, r.h
	local scale = (w * h) / total
	local i = 1
	while i <= #weights do
		local side = math.min(w, h)
		local row = { weights[i] }
		local sum = weights[i]
		local j = i + 1
		while j <= #weights do
			local trial = table.clone(row)
			table.insert(trial, weights[j])
			if worst(trial, sum + weights[j], side, scale) <= worst(row, sum, side, scale) then
				row = trial
				sum += weights[j]
				j += 1
			else
				break
			end
		end
		-- (The row laid along the shorter side; what's left beyond it.)
		local area = sum * scale
		if w >= h then
			local rw = if j > #weights then w else area / h
			local yy = y
			for k, v in row do
				local rh = if k == #row then y + h - yy else v * scale / rw
				table.insert(out, { x = x, y = yy, w = rw, h = rh })
				yy += rh
			end
			x += rw
			w -= rw
		else
			local rh = if j > #weights then h else area / w
			local xx = x
			for k, v in row do
				local rw = if k == #row then x + w - xx else v * scale / rh
				table.insert(out, { x = xx, y = y, w = rw, h = rh })
				xx += rw
			end
			y += rh
			h -= rh
		end
		i = j
	end
	return out
end

local function inset(r: Rect, d: number): Rect
	return { x = r.x + d, y = r.y + d, w = math.max(0, r.w - 2 * d), h = math.max(0, r.h - 2 * d) }
end

-- A group's grid: the columns that give cells nearest a name's shape.
local function grid(n: number, w: number, h: number): (number, number, number, number)
	if n <= 0 then
		return 0, 0, w, 0
	end
	local best, best_c = math.huge, 1
	for c = 1, n do
		local rows = math.ceil(n / c)
		local cw, ch = w / c, h / rows
		local fit = math.min(ch, M.CELL_H_MAX)
		-- (Too short a cell to name a thread costs most; then a cell far
		-- from a name's width; then height left over.)
		local cost = math.abs(math.log(cw / M.CELL_W_IDEAL)) * 2
			+ (if ch < M.CELL_H_MIN then (M.CELL_H_MIN - ch) else 0)
			+ (if cw < 80 then (80 - cw) * 0.2 else 0)
			+ (h - fit * rows) / math.max(1, h) * 0.5
		if cost < best - 1e-9 then
			best, best_c = cost, c
		end
	end
	local rows = math.ceil(n / best_c)
	return best_c, rows, w / best_c, math.min(h / rows, M.CELL_H_GROW)
end

function M.layout(scene: any, width: number): Layout
	local machines_in = scene.machines or {}
	-- How many cells each machine needs: its threads, each group's lead row.
	local need: { number } = {}
	local cells = 0
	for i, m in machines_in do
		local n = 0
		for _, g in m.groups or {} do
			n += #(g.members or {}) + (if g.lead then 1.2 else 0)
		end
		need[i] = n
		cells += n
	end
	local w = math.max(1, width)
	local h = math.clamp(math.ceil(cells * M.CELL_AREA / w + #machines_in * M.HEAD * 2 + M.PAD * 2), M.MIN_H, M.MAX_H)
	h = math.max(h, math.min(M.MAX_H, math.floor(w * 0.45)))

	local out: Layout = { w = w, h = h, machines = {}, groups = {}, nodes = {}, index = {}, lines = {}, sig = "" }
	local sig = { tostring(math.floor(w)) }

	-- Machines: an idle one still gets a heading's room.
	local weights: { number } = {}
	for i in machines_in do
		weights[i] = math.max(need[i], 2) + 2
	end
	local field = { x = M.PAD - M.GAP / 2, y = M.PAD - M.GAP / 2, w = w - 2 * M.PAD + M.GAP, h = h - 2 * M.PAD + M.GAP }
	local rects = M.treemap(weights, field)

	for mi, m in machines_in do
		local r = inset(rects[mi], M.GAP / 2)
		local mm: Machine = { id = m.id, name = m.name or "", rect = r, first_group = #out.groups + 1, groups = 0, count = 0 }
		table.insert(out.machines, mm)
		table.insert(sig, `m:{m.id}`)
		local body = { x = r.x, y = r.y + M.HEAD, w = r.w, h = math.max(0, r.h - M.HEAD) }
		-- Groups stacked down the body, each its full width, each as tall
		-- as its rows ask (the room left shared out the same way).
		local groups_in = m.groups or {}
		local bw = body.w - 8
		-- (Each its least first, then the room left shared by rows, none
		-- past what its cells can use: what's left over sits at the foot.)
		local want: { number } = {}
		local cap: { number } = {}
		local rows_of: { number } = {}
		local total = 0
		for gi, g in groups_in do
			local n = #(g.members or {})
			local cols = math.max(1, math.min(n, math.floor((bw - M.CELL_X) / M.CELL_W_IDEAL)))
			local rows = if n > 0 then math.ceil(n / cols) else 0
			local head = (if g.lead then M.LEAD_H else 4) + M.GROUP_GAP
			rows_of[gi] = rows
			want[gi] = rows * M.CELL_H_MIN + head
			cap[gi] = rows * M.CELL_H_GROW + head
			total += want[gi]
		end
		local room = body.h - 4
		if total > room then
			for gi in groups_in do
				want[gi] = want[gi] / total * room
			end
		else
			local extra = room - total
			for _ = 1, 3 do
				local open_rows = 0
				for gi in groups_in do
					if want[gi] < cap[gi] - 0.01 then
						open_rows += rows_of[gi]
					end
				end
				if open_rows == 0 or extra < 0.5 then
					break
				end
				local given = 0
				for gi in groups_in do
					if want[gi] < cap[gi] - 0.01 then
						local add = math.min(cap[gi] - want[gi], extra * rows_of[gi] / open_rows)
						want[gi] += add
						given += add
					end
				end
				extra -= given
			end
		end
		local grects: { Rect } = {}
		local yy = body.y
		for gi in groups_in do
			grects[gi] = { x = body.x + 4, y = yy, w = bw, h = want[gi] }
			yy += want[gi]
		end
		for gi, g in groups_in do
			local r0 = grects[gi]
			local gr = { x = r0.x, y = r0.y, w = r0.w, h = math.max(0, r0.h - M.GROUP_GAP) }
			local members = g.members or {}
			local lead_t = g.lead
			local top = gr.y + (if lead_t then M.LEAD_H else 4)
			local gx = gr.x + (if lead_t then M.CELL_X else 4)
			local cols, rows, cw, ch = grid(#members, gr.x + gr.w - gx, gr.y + gr.h - top)
			local group: Group = {
				id = g.id,
				machine = mi,
				rect = gr,
				lead = nil,
				first = #out.nodes + 1,
				count = #members,
				cols = cols,
				rows = rows,
				cell_w = cw,
				cell_h = ch,
				gx = gx,
				gy = top,
				trunk_x = gr.x + M.TRUNK,
				trunk_y0 = gr.y + M.LEAD_H / 2,
				trunk_y1 = gr.y + M.LEAD_H / 2,
				hue = (lead_t and lead_t.hue) or nil,
				rails = {},
			}
			table.insert(out.groups, group)
			local gidx = #out.groups
			mm.groups += 1
			table.insert(sig, `g:{g.id}`)
			if lead_t then
				local n: Node = {
					id = if g.ghost then `ghost:{lead_t.id}@{m.id}` else lead_t.id,
					title = lead_t.title or "",
					kind = lead_t.kind or "orchestrator",
					hue = lead_t.hue,
					machine = mi,
					group = gidx,
					x = group.trunk_x,
					y = gr.y + M.LEAD_H / 2,
					cx = gr.x + M.TRUNK + 9,
					cy = gr.y,
					cw = gr.w - M.TRUNK - 9,
					ch = M.LEAD_H,
					lead = true,
					ghost = g.ghost == true,
				}
				table.insert(out.nodes, n)
				out.index[n.id] = #out.nodes
				group.lead = #out.nodes
				group.first = #out.nodes + 1
				table.insert(sig, `l:{n.id}`)
			end
			for k, t in members do
				local col = (k - 1) % math.max(1, cols)
				local row = (k - 1) // math.max(1, cols)
				local x0 = gx + col * cw
				local y0 = top + row * ch
				local n: Node = {
					id = t.id,
					title = t.title or "",
					kind = t.kind or "thread",
					hue = t.hue,
					machine = mi,
					group = gidx,
					x = x0 + 6,
					y = y0 + ch / 2 + (if lead_t then 2 else 0),
					cx = x0 + 14,
					cy = y0,
					cw = cw - 16,
					ch = ch,
					lead = false,
					ghost = false,
				}
				table.insert(out.nodes, n)
				out.index[n.id] = #out.nodes
				mm.count += 1
				table.insert(sig, `n:{t.id}`)
			end
			-- The comb: a rail over each row (in the gap above its dots), the
			-- trunk down to the last.
			if lead_t and #members > 0 then
				for row = 0, rows - 1 do
					local last = math.min(#members, (row + 1) * cols)
					local lastn = out.nodes[group.first + last - 1]
					local ry = top + row * ch + 2
					table.insert(group.rails, ry)
					table.insert(group.rails, lastn.x)
					group.trunk_y1 = ry
				end
			end
		end
	end

	-- A lead to its ghosts; what the Brain dump handed parts to. Each runs
	-- in the gaps: up out of its group, across to the gutter beside its
	-- machine (or the field's edge), along it, and into the other.
	local function route(a: number, b: number): { number }
		local na, nb = out.nodes[a], out.nodes[b]
		local ga, gb = out.groups[na.group], out.groups[nb.group]
		local ma, mb = out.machines[na.machine].rect, out.machines[nb.machine].rect
		local ya = ga.rect.y - M.GROUP_GAP / 2
		local yb = gb.rect.y - M.GROUP_GAP / 2
		local gx
		if mb.x >= ma.x + ma.w - 1 then
			gx = ma.x + ma.w + M.GAP / 2
		elseif ma.x >= mb.x + mb.w - 1 then
			gx = ma.x - M.GAP / 2
		else
			gx = math.min(ma.x, mb.x) - M.GAP / 2
		end
		if na.machine == nb.machine then
			gx = ma.x + 2
		end
		return { na.x, na.y, na.x, ya, gx, ya, gx, yb, nb.x, yb, nb.x, nb.y }
	end
	for _, n in out.nodes do
		if n.ghost then
			local real = string.match(n.id, "^ghost:(.-)@")
			local a = real and out.index[real]
			if a then
				local b = out.index[n.id]
				table.insert(out.lines, { a = a, b = b, kind = "away", hue = n.hue, pts = route(a, b) })
			end
		end
	end
	for _, l in scene.links or {} do
		local a, b = out.index[l.a], out.index[l.b]
		if a and b and a ~= b then
			table.insert(out.lines, { a = a, b = b, kind = l.kind or "routed", hue = l.hue, pts = route(a, b) })
			table.insert(sig, `x:{l.a}>{l.b}`)
		end
	end
	out.sig = table.concat(sig, "|")
	return out
end

local function inside(r: Rect, x: number, y: number): boolean
	return x >= r.x and x < r.x + r.w and y >= r.y and y < r.y + r.h
end

-- The node at (x, y), in the layout's space: its machine, its group, then
-- the cell by arithmetic. A machine's heading answers as nil (its own
-- hit is the caller's: a header).
function M.cell_at(l: Layout, x: number, y: number): number?
	for _, g in l.groups do
		local r = g.rect
		if inside(r, x, y) then
			if g.lead and y < g.gy then
				return g.lead
			end
			if g.count == 0 or g.cols == 0 then
				return nil
			end
			local col = math.floor((x - g.gx) / g.cell_w)
			local row = math.floor((y - g.gy) / g.cell_h)
			if col < 0 or row < 0 or col >= g.cols or row >= g.rows then
				return nil
			end
			local k = row * g.cols + col
			if k >= g.count then
				return nil
			end
			return g.first + k
		end
	end
	return nil
end

-- Node i's line from its lead, as drawn (flat x, y, …): down the trunk,
-- along its row's rail, down its tick. A lead's own, or a thread of no
-- lead: nil.
function M.path(l: Layout, i: number): { number }?
	local n = l.nodes[i]
	if not n or n.lead then
		return nil
	end
	local g = l.groups[n.group]
	if not g or not g.lead then
		return nil
	end
	local lead = l.nodes[g.lead]
	local k = i - g.first
	local row = k // math.max(1, g.cols)
	local ry = g.rails[row * 2 + 1] or n.y
	return { lead.x, lead.y, g.trunk_x, ry, n.x, ry, n.x, n.y }
end

return M

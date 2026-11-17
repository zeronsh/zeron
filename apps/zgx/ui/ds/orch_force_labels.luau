--!native
-- Where the force Graph (ds/orch_force) writes its names, worked out once
-- the web rests (never a frame of the settling), in world units.
--
-- The Brain's and each orchestrator's are fixed: centred just under the
-- mark, an orchestrator's wrapped to two lines (never cut so short it
-- can't be told), whatever crowds it. Every thread's is placed round its
-- own mark, as the first Graph (ds/orch_map_graph) placed them: on a side
-- free of every other name, mark and line, away from its hub first; one
-- line, else two, else (only then) shortened; a step further out if close
-- in is taken; left out only where nothing is free (its card names it).
-- After a drag, a name whose node didn't move keeps its place while it's
-- still free: only what moved, and what it now covers, is placed again.
--
--   labels.place(input) -> { [number]: Label }
--
-- input: {
--   nodes: { { id, kind = "brain" | "orchestrator" | "chat", x, y, r,
--              title, hx?, hy? (its hub), rank? (lower first) } },
--   links: { { ax, ay, bx, by } },
--   measure: (text, hub: boolean) -> width,
--   prev: { [string]: Label }?   the last placement, by id
--   hub_r: number   an orchestrator mark's radius (its name hangs under it)
-- }

local M = {}

export type Label = {
	lines: { string },
	-- Anchor x, top y, and how the lines align on the anchor.
	x: number,
	y: number,
	align: "center" | "start" | "end",
	w: number,
	h: number,
	-- The box it takes (with its ground).
	x0: number,
	y0: number,
	x1: number,
	y1: number,
	fixed: boolean,
	-- Where its node was when it was placed.
	nx: number,
	ny: number,
	-- Set off from its mark: a fine leader back to it (from, to).
	leader: { number }?,
}

M.LH_HUB = 16
M.LH = 15
-- A name's ground round its text.
M.PAD_X = 4
M.PAD_Y = 1
-- An orchestrator's name: lines at most this wide; a thread's one line at
-- most this wide before it wraps, its two lines this wide.
M.HUB_W = 170
M.ONE_W = 190
M.TWO_W = 120
-- How far a thread's name sits off its mark, close in then stepped out.
M.GAP = 4
M.STEPS = { 0, 10, 24, 44 }
-- Whether a name keeps off the lines.
M.LINES = true
-- The grid the marks, names and lines are looked up in.
local CELL = 48

-- Words into lines no wider than `most`; the last line cut with an
-- ellipsis where `max_lines` lines don't hold it all.
function M.wrap(text: string, most: number, max_lines: number, measure: (string) -> number): { string }
	local words = string.split(text, " ")
	local lines: { string } = {}
	local cur = ""
	local i = 1
	while i <= #words do
		local w = words[i]
		local try = if cur == "" then w else cur .. " " .. w
		if measure(try) <= most or cur == "" then
			cur = try
			i += 1
		else
			table.insert(lines, cur)
			cur = ""
			if #lines == max_lines then
				break
			end
		end
	end
	if cur ~= "" and #lines < max_lines then
		table.insert(lines, cur)
		cur = ""
	end
	-- (What's left over: the last line takes it, cut to fit.)
	local rest = {}
	for k = i, #words do
		table.insert(rest, words[k])
	end
	local last = lines[#lines] or ""
	if #rest > 0 or measure(last) > most then
		local full = if #rest > 0 then last .. " " .. table.concat(rest, " ") else last
		local n = utf8.len(full) or #full
		while n > 1 do
			local cut = string.sub(full, 1, (utf8.offset(full, n) or (#full + 1)) - 1)
			cut = (string.gsub(cut, "%s+$", "")) .. "…"
			if measure(cut) <= most then
				lines[math.max(1, #lines)] = cut
				return lines
			end
			n -= 1
		end
		lines[math.max(1, #lines)] = "…"
	end
	return lines
end

-- A shortened one line: at the last whole word that fits `chars`, an
-- ellipsis after it.
local function shortened(text: string, chars: number): string?
	local n = utf8.len(text) or #text
	if n <= chars then
		return nil
	end
	local head = string.sub(text, 1, (utf8.offset(text, chars + 1) or (#text + 1)) - 1)
	local cut = string.match(head, "^(.*%S)[%s%-]+[^%s%-]*$") or head
	return cut .. "…"
end

-- The grid: boxes (marks, names) and lines, by cell (a line in every cell
-- it passes through).
type Grid = { boxes: { [number]: { { any } } }, segs: { [number]: { { any } } } }
local function key(cx: number, cy: number): number
	return (cx + 32768) * 65536 + (cy + 32768)
end
local function add_box(g: Grid, b: { any })
	for cx = math.floor(b[1] / CELL), math.floor(b[3] / CELL) do
		for cy = math.floor(b[2] / CELL), math.floor(b[4] / CELL) do
			local k = key(cx, cy)
			local list = g.boxes[k]
			if not list then
				list = {}
				g.boxes[k] = list
			end
			table.insert(list, b)
		end
	end
end
local function add_seg(g: Grid, sg: { any })
	local ax, ay, bx, by = sg[1], sg[2], sg[3], sg[4]
	local len = math.sqrt((bx - ax) ^ 2 + (by - ay) ^ 2)
	local steps = math.max(1, math.ceil(len / (CELL / 3)))
	local seen: { [number]: boolean } = {}
	for k = 0, steps do
		local t = k / steps
		local cx, cy = math.floor((ax + (bx - ax) * t) / CELL), math.floor((ay + (by - ay) * t) / CELL)
		-- (Its cell and the neighbours a corner-cut could touch.)
		for ddx = -1, 1 do
			for ddy = -1, 1 do
				local kk = key(cx + ddx, cy + ddy)
				if not seen[kk] then
					seen[kk] = true
					local list = g.segs[kk]
					if not list then
						list = {}
						g.segs[kk] = list
					end
					table.insert(list, sg)
				end
			end
		end
	end
end
-- Where a segment crosses a box: its entry and exit along it (0–1), or nil.
local function clip(ax: number, ay: number, bx: number, by: number, x0: number, y0: number, x1: number, y1: number): (number?, number?)
	local t0, t1 = 0, 1
	local dx, dy = bx - ax, by - ay
	local ps = { -dx, dx, -dy, dy }
	local qs = { ax - x0, x1 - ax, ay - y0, y1 - ay }
	for k = 1, 4 do
		local p, q = ps[k], qs[k]
		if p == 0 then
			if q < 0 then
				return nil, nil
			end
		else
			local r = q / p
			if p < 0 then
				t0 = math.max(t0, r)
			else
				t1 = math.min(t1, r)
			end
			if t0 > t1 then
				return nil, nil
			end
		end
	end
	return t0, t1
end
M.clip = clip
-- Whether the box is free: of every box but its own node's (`own`), and of
-- every line but its own node's where they leave its mark (within `oreach`).
local stamp = 0
local function free(g: Grid, x0: number, y0: number, x1: number, y1: number, own: number?, ox: number, oy: number, oreach: number): boolean
	stamp += 1
	for cx = math.floor(x0 / CELL), math.floor(x1 / CELL) do
		for cy = math.floor(y0 / CELL), math.floor(y1 / CELL) do
			local k = key(cx, cy)
			for _, b in g.boxes[k] or {} do
				if b[5] ~= own and x0 < b[3] and x1 > b[1] and y0 < b[4] and y1 > b[2] then
					return false
				end
			end
			if M.LINES then
				for _, sg in g.segs[k] or {} do
					if sg[7] ~= stamp then
						sg[7] = stamp
						local t0, t1 = clip(sg[1], sg[2], sg[3], sg[4], x0, y0, x1, y1)
						if t0 then
							local mine = sg[5] == own or sg[6] == own
							if not mine then
								return false
							end
							-- (Its own line: only where it leaves its mark.)
							local r2 = oreach * oreach
							local ex, ey = sg[3] - sg[1], sg[4] - sg[2]
							local px, py = sg[1] + ex * t0, sg[2] + ey * t0
							local qx, qy = sg[1] + ex * (t1 :: number), sg[2] + ey * (t1 :: number)
							if (px - ox) ^ 2 + (py - oy) ^ 2 > r2 or (qx - ox) ^ 2 + (qy - oy) ^ 2 > r2 then
								return false
							end
						end
					end
				end
			end
		end
	end
	return true
end

local function make(lines: { string }, x: number, y: number, align: "center" | "start" | "end", w: number, lh: number, fixed: boolean, nx: number, ny: number): Label
	local h = lh * #lines
	local x0 = if align == "center" then x - w / 2 elseif align == "start" then x else x - w
	return {
		lines = lines,
		x = x,
		y = y,
		align = align,
		w = w,
		h = h,
		x0 = x0 - M.PAD_X,
		y0 = y - M.PAD_Y,
		x1 = x0 + w + M.PAD_X,
		y1 = y + h + M.PAD_Y,
		fixed = fixed,
		nx = nx,
		ny = ny,
	}
end

-- A placement begun (the marks, lines and fixed names laid down) and taken
-- on a slice at a time: `M.run(job, seconds)` places threads' names till
-- the time's up, true once all are.
function M.start(input: any): any
	local nodes = input.nodes
	local measure = input.measure
	local prev = input.prev or {}
	local out: { [number]: Label } = {}
	local g: Grid = { boxes = {}, segs = {} }
	-- Marks: each node's box (a little past its ring).
	for i, n in nodes do
		local r = n.r + 2
		add_box(g, { n.x - r, n.y - r, n.x + r, n.y + r, i })
	end
	-- Lines (each knows its two ends).
	for _, l in input.links do
		add_seg(g, { l.ax, l.ay, l.bx, l.by, l.a, l.b })
	end
	-- The fixed ones first: the Brain's and each orchestrator's, under them.
	for i, n in nodes do
		if n.kind ~= "chat" then
			local hub = true
			local function m(t: string): number
				return measure(t, hub)
			end
			local lines = if n.kind == "brain" then { n.title } else M.wrap(n.title, M.HUB_W, 2, m)
			local w = 0
			for _, t in lines do
				w = math.max(w, m(t))
			end
			local lab = make(lines, n.x, n.y + n.r + 3, "center", w, M.LH_HUB, true, n.x, n.y)
			out[i] = lab
			add_box(g, { lab.x0, lab.y0, lab.x1, lab.y1, -i })
		end
	end
	-- Threads, the most pressing first.
	local order: { number } = {}
	for i, n in nodes do
		if n.kind == "chat" then
			table.insert(order, i)
		end
	end
	table.sort(order, function(a: number, b: number): boolean
		local ra, rb = nodes[a].rank or 0, nodes[b].rank or 0
		if ra ~= rb then
			return ra < rb
		end
		return a < b
	end)
	local widths: { [string]: number } = input.widths or {}
	local job: any = { nodes = nodes, order = order, at = 1, out = out, g = g, prev = prev, measure = measure }
	-- (Measured with the measure given to the slice running: a canvas lives
	-- for its own drawing only.)
	job.w_of = function(t: string): number
		local w = widths[t]
		if not w then
			w = job.measure(t, false)
			widths[t] = w
		end
		return w
	end
	return job
end

function M.run(job: any, budget: number?, measure: any?): boolean
	if measure then
		job.measure = measure
	end
	local nodes, order, out, g, prev, w_of = job.nodes, job.order, job.out, job.g, job.prev, job.w_of
	local t_end = if budget then os.clock() + budget else math.huge
	while job.at <= #order do
		if os.clock() >= t_end then
			return false
		end
		local i = order[job.at]
		job.at += 1
		local n = nodes[i]
		local x, y = n.x, n.y
		local reach = n.r + 10
		local placed: any = nil
		-- (Where it was, if its node didn't move and it's still free.)
		local was: any = prev[n.id]
		if was and not was.fixed and math.abs(was.nx - x) < 0.01 and math.abs(was.ny - y) < 0.01 then
			if free(g, was.x0, was.y0, was.x1, was.y1, i, x, y, reach) then
				placed = was
			end
		end
		if not placed then
			-- (Its forms: one line, two, then shortened.)
			local forms: { { string } } = {}
			local title = n.title or ""
			if w_of(title) <= M.ONE_W then
				table.insert(forms, { title })
			end
			local two = M.wrap(title, M.TWO_W, 2, w_of)
			if #two == 2 or #forms == 0 then
				table.insert(forms, two)
			end
			for _, chars in { 18, 12, 9 } do
				local s = shortened(title, chars)
				if s then
					table.insert(forms, { s })
				end
			end
			-- (Its sides, away from its hub first.)
			local ax, ay = 0, 1
			if n.hx then
				local dx, dy = x - n.hx, y - n.hy
				local d = math.sqrt(dx * dx + dy * dy)
				if d > 0.01 then
					ax, ay = dx / d, dy / d
				end
			end
			local dirs = {}
			for k = 0, 15 do
				local a = k * math.pi / 8
				local dx, dy = math.cos(a), math.sin(a)
				table.insert(dirs, { dx, dy, if dx > 0.3 then "start" elseif dx < -0.3 then "end" else "center" })
			end
			table.sort(dirs, function(a: any, b: any): boolean
				return a[1] * ax + a[2] * ay > b[1] * ax + b[2] * ay
			end)
			for _, form in forms do
				local w = 0
				for _, t in form do
					w = math.max(w, w_of(t))
				end
				local h = M.LH * #form
				for _, step in M.STEPS do
					local r = n.r + M.GAP + step
					for _, d in dirs do
						local dx: number, dy: number, align: any = d[1] :: any, d[2] :: any, d[3]
						local lx = x + dx * r
						local ly
						if dy > 0.5 then
							ly = y + r
						elseif dy < -0.5 then
							ly = y - r - h
						elseif dy > 0 then
							ly = y + dy * r - h / 2 + 4
						elseif dy < 0 then
							ly = y + dy * r - h / 2 - 4
						else
							ly = y - h / 2
						end
						local lab = make(form, lx, ly, align :: any, w, M.LH, false, x, y)
						if free(g, lab.x0, lab.y0, lab.x1, lab.y1, i, x, y, reach + step) then
							if step >= 18 then
								-- (From its ring's edge to the name's nearest point.)
								local qx, qy = math.clamp(x, lab.x0, lab.x1), math.clamp(y, lab.y0, lab.y1)
								local gx, gy = qx - x, qy - y
								local gd = math.max(0.01, math.sqrt(gx * gx + gy * gy))
								lab.leader = { x + gx / gd * (n.r + 1), y + gy / gd * (n.r + 1), qx - gx / gd * 2, qy - gy / gd * 2 }
							end
							placed = lab
							break
						end
					end
					if placed then
						break
					end
				end
				if placed then
					break
				end
			end
		end
		if placed then
			out[i] = placed
			add_box(g, { placed.x0, placed.y0, placed.x1, placed.y1, -i })
		end
	end
	return true
end

-- All at once.
function M.place(input: any): { [number]: Label }
	local job = M.start(input)
	M.run(job, nil)
	return job.out
end

return M

--!nonstrict
-- The force Graph's balance (ds/orch_force_sim, ds/orch_force_labels),
-- measured with no window: each fleet shape laid out and fitted to each
-- frame, then how well it fills it. Prints `FORCEBAL {json}` a layout:
--   cover   the frame's share within NEAR points of a mark or a name
--   empty   the largest clear disc's radius, over the frame's short side
--   nn_cv   nearest-neighbour distances' spread (std / mean)
--   shape   |ln| of the picture's aspect over the frame's
--   overlap names over names; over marks; clipped (past the frame);
--   unnamed threads meant to be named and left out; zoom.

local sim = require("@orchestrators/ds/orch_force_sim")
local labels = require("@orchestrators/ds/orch_force_labels")
local draw = require("@orchestrators/ds/orch_force_draw")

local R_BRAIN, R_HUB, R_RING = draw.BRAIN_D / 2, draw.HUB_D / 2, draw.R_RING
local MIN_FIT = 0.75
local NEAR = 48

-- Names as they draw: 11.5 points (a thread's), 12.5 bold (a hub's).
local function measure(t: string, hub: boolean, k: number): number
	return (utf8.len(t) or #t) * (if hub then 7.3 else 6.2) * k
end

local VPN = {
	"Windows build worker (x16-win, Claude)", "per-app exits", "Per-app exits: 2 fixes + renumber 0068 + tests", "Crew thread 5b9c4d",
	"dscp marking", "iOS app (K2) (Claude)", "new UDP family", "hy2 HoL fix", "Deploy lane (Opus)", "RU routing: whitelist survival + CDN edge",
	"tcp-brutal", "admin panel", "Auto-install no-click (Mac/Linux)", "control alloc fix", "whitelist mode", "spoof-channel research",
	"proactive probes", "Proactive probes: renumber 0067 + migration", "Android updater (Opus)", "macOS install/update proof (Claude)",
	"Moscow RU node (msk-2)", "Telemetry v2 (Opus 5.5 High)", "Protocols CDN/ShadowTLS (Opus)", "Throughput regression bench", "Exporter image",
}
local FIXES = {
	"Orchestrator creation fixes", "Control Room tabs", "Android crash sending to an orchestrator", "Per-chat Cloud machine size",
	"Merge main 0.3.8 into orchestrators", "iOS design options: orchestrators + Control Room", "Board redesign: 3 columns, derived states",
	"Cursor missing from menus", "Orchestrators as a built-in plugin", "Force graph hover intent", "Sidebar perf: whole-list computeds", "Graph polish",
}
-- (Mixed: working, waiting, idle, finished; the screenshot had 6 working.)
local MIX = { "idle", "working", "idle", "done", "waiting", "idle", "idle", "working", "done", "idle", "input", "idle", "working", "idle", "done", "idle", "waiting", "idle" }

local function hub_of(name: string, n: number, titles: { string }?, offset: number?): any
	local list = {}
	for i = 1, n do
		local t = if titles then titles[(i - 1) % #titles + 1] else `{name} thread {i}`
		table.insert(list, { title = t, state = MIX[(i + (offset or 0) - 1) % #MIX + 1] })
	end
	return { name = name, crew = list }
end

local SHAPES: { [string]: any } = {
	screenshot = { hubs = { hub_of("VPN Orchestrator", 25, VPN), hub_of("Zeron fixes", 12, FIXES, 3) }, own = 1 },
	sparse = { hubs = { hub_of("Release", 3, nil, 1), hub_of("Docs", 2, nil, 4) }, own = 0 },
	one40 = { hubs = { hub_of("Engine", 40, VPN, 2) }, own = 0 },
	uneven4 = { hubs = { hub_of("VPN Orchestrator", 30, VPN), hub_of("Zeron fixes", 12, FIXES, 3), hub_of("Docs", 5, nil, 5), hub_of("Release", 2, nil, 7) }, own = 2 },
	seven = { hubs = { hub_of("A team", 9, VPN, 1), hub_of("B team", 6, FIXES, 2), hub_of("C team", 4, nil, 3), hub_of("D team", 12, VPN, 4), hub_of("E team", 3, nil, 5), hub_of("F team", 7, FIXES, 6), hub_of("G team", 5, nil, 7) }, own = 1 },
}
local SHAPE_ORDER = { "screenshot", "sparse", "one40", "uneven4", "seven" }
local FRAMES = { { 1300, 700 }, { 1500, 900 }, { 700, 600 } }

local ACTIVE: { [string]: boolean } = { working = true, waiting = true, input = true, approval = true, failed = true }

-- A variant: sim options, and the Graph's own choices.
type Variant = { name: string, wide: boolean?, space: boolean?, tight: boolean?, fold: number?, quiet: boolean?, min_fit: number?, fill: boolean?, turn: boolean?, flat: boolean?, turn_below: number?, stretch_most: number?, even: boolean?, even_share: number?, spoke: number? }

-- The web for a shape: nodes (the Brain first), links, and per node its
-- kind, title, state, radius.
local function build(shape: any, v: Variant): any
	local nodes, links, meta = {}, {}, {}
	local function add(id: string, parent: number, kind: string, title: string, state: string?): number
		table.insert(nodes, { id = id, parent = if parent > 0 then parent else nil, fixed = kind == "brain" })
		local r = if kind == "brain" then R_BRAIN elseif kind == "orchestrator" then R_HUB else R_RING
		table.insert(meta, { id = id, kind = kind, title = title, state = state, r = r })
		local i = #nodes
		if parent > 0 then
			table.insert(links, { a = parent, b = i, strong = true, hub = kind == "orchestrator" })
		end
		return i
	end
	local brain = add("brain", 0, "brain", "Brain")
	for k, h in shape.hubs do
		local hi = add(`h{k}`, brain, "orchestrator", h.name)
		local folded = 0
		for j, c in h.crew do
			if v.fold and not ACTIVE[c.state] then
				folded += 1
			else
				add(`h{k}c{j}`, hi, "chat", c.title, c.state)
			end
		end
		if v.fold and folded > 0 then
			if folded >= v.fold then
				add(`h{k}fold`, hi, "fold", `{folded}`, "idle")
			else
				-- (Too few to fold: they stand alone.)
				for j, c in h.crew do
					if not ACTIVE[c.state] then
						add(`h{k}c{j}`, hi, "chat", c.title, c.state)
					end
				end
			end
		end
	end
	for j = 1, shape.own do
		add(`own{j}`, brain, "chat", if j == 1 then "Corporate Zeron Reins Strategy" else `Brain note {j}`, "working")
	end
	return { nodes = nodes, links = links, meta = meta }
end

-- Laid out for a frame, fitted, its names placed at the fitted scale;
-- what the frame shows (screen points).
local function lay(shape: any, v: Variant, w: number, h: number): any
	local web = build(shape, v)
	local tb, sm, es = sim.TURN_BELOW, sim.STRETCH_MOST, sim.EVEN_SHARE
	sim.EVEN_SHARE = v.even_share or es
	sim.TURN_BELOW = v.turn_below or tb
	sim.STRETCH_MOST = v.stretch_most or sm
	local s = sim.new(web.nodes, web.links, 7, { aspect = w / h, dense = 1, wide = v.wide, space = v.space, tight = v.tight, turn = v.turn, flat = v.flat or v.fill, even = v.even })
	if v.fill then
		sim.stretch(s, w, h - 30, 70)
	end
	sim.place(s, nil)
	sim.TURN_BELOW, sim.STRETCH_MOST, sim.EVEN_SHARE = tb, sm, es
	sim.wake(s, nil)
	for _ = 1, 4000 do
		sim.step(s)
		if sim.still(s) then
			break
		end
	end
	local min_fit = v.min_fit or MIN_FIT
	local z, labs: any = 1, {}
	for _ = 1, 4 do
		local k = 1 / z
		local ns = {}
		for i, m in web.meta do
			local p = s.parent[i]
			local quiet = m.kind == "fold"
			local spare = v.quiet and m.kind == "chat" and not ACTIVE[m.state]
			table.insert(ns, { id = m.id, kind = if m.kind == "fold" then "chat" else m.kind, x = s.x[i], y = s.y[i], r = m.r, title = m.title,
				hx = if p > 0 then s.x[p] else nil, hy = if p > 0 then s.y[p] else nil, rank = if ACTIVE[m.state or ""] then 0 else 3, quiet = quiet, spare = spare })
		end
		local ls = {}
		for _, l in web.links do
			table.insert(ls, { a = l.a, b = l.b, ax = s.x[l.a], ay = s.y[l.a], bx = s.x[l.b], by = s.y[l.b] })
		end
		labs = labels.place({ scale = k, nodes = ns, links = ls, measure = function(t: string, hub: boolean): number
			return measure(t, hub, k)
		end })
		local x0, y0, x1, y1 = sim.bounds(s)
		for _, l: any in labs do
			x0, y0, x1, y1 = math.min(x0, l.x0), math.min(y0, l.y0), math.max(x1, l.x1), math.max(y1, l.y1)
		end
		local nz = math.clamp(math.min((w - 48) / math.max(1, x1 - x0), (h - 78) / math.max(1, y1 - y0)), min_fit, 1.4)
		local done = math.abs(nz - z) / z < 0.02
		z = nz
		if done then
			break
		end
	end
	local x0, y0, x1, y1 = sim.bounds(s)
	for _, l: any in labs do
		x0, y0, x1, y1 = math.min(x0, l.x0), math.min(y0, l.y0), math.max(x1, l.x1), math.max(y1, l.y1)
	end
	local vx, vy = w / 2 - (x0 + x1) / 2 * z, (h - 30) / 2 - (y0 + y1) / 2 * z
	local out = { marks = {}, names = {}, links = {}, w = w, h = h, zoom = z, bw = (x1 - x0) * z, bh = (y1 - y0) * z, steps = s.steps }
	for i, m in web.meta do
		table.insert(out.marks, { x = s.x[i] * z + vx, y = s.y[i] * z + vy, r = m.r * z, kind = m.kind, state = m.state, title = m.title, named = labs[i] ~= nil })
	end
	for i, l: any in labs do
		table.insert(out.names, { x0 = l.x0 * z + vx, y0 = l.y0 * z + vy, x1 = l.x1 * z + vx, y1 = l.y1 * z + vy, lines = l.lines, x = l.x * z + vx, y = l.y * z + vy, align = l.align, fixed = l.fixed, i = i })
	end
	for _, l in web.links do
		table.insert(out.links, { a = l.a, b = l.b })
	end
	return out
end

-- How well a layout fills its frame.
local function measure_layout(L: any, v: Variant): any
	local w, h = L.w, L.h
	local CELL = 8
	local near, total, worst = 0, 0, 0
	for gx = CELL / 2, w, CELL do
		for gy = CELL / 2, h, CELL do
			local best = math.huge
			for _, m in L.marks do
				local d = math.sqrt((gx - m.x) ^ 2 + (gy - m.y) ^ 2) - m.r
				best = math.min(best, d)
			end
			for _, b in L.names do
				local dx = math.max(b.x0 - gx, 0, gx - b.x1)
				local dy = math.max(b.y0 - gy, 0, gy - b.y1)
				best = math.min(best, math.sqrt(dx * dx + dy * dy))
			end
			-- (A clear disc there can't pass the frame's edge.)
			local edge = math.min(gx, gy, w - gx, h - gy)
			worst = math.max(worst, math.min(best, edge))
			total += 1
			if best <= NEAR then
				near += 1
			end
		end
	end
	local nn = {}
	for i, a in L.marks do
		local best = math.huge
		for j, b in L.marks do
			if i ~= j then
				best = math.min(best, math.sqrt((a.x - b.x) ^ 2 + (a.y - b.y) ^ 2))
			end
		end
		if best < math.huge then
			table.insert(nn, best)
		end
	end
	local mean, var = 0, 0
	for _, d in nn do
		mean += d
	end
	mean /= math.max(1, #nn)
	for _, d in nn do
		var += (d - mean) ^ 2
	end
	local cv = if #nn > 1 then math.sqrt(var / #nn) / mean else 0
	local over, over_marks, clipped, unnamed = 0, 0, 0, 0
	for a, la in L.names do
		for b, lb in L.names do
			if a < b and la.x0 + 1 < lb.x1 and la.x1 - 1 > lb.x0 and la.y0 + 1 < lb.y1 and la.y1 - 1 > lb.y0 then
				over += 1
			end
		end
		for j, m in L.marks do
			if j ~= la.i and la.x0 + 1 < m.x + m.r and la.x1 - 1 > m.x - m.r and la.y0 + 1 < m.y + m.r and la.y1 - 1 > m.y - m.r then
				over_marks += 1
			end
		end
		if la.x0 < -0.5 or la.y0 < -0.5 or la.x1 > w + 0.5 or la.y1 > h + 0.5 then
			clipped += 1
		end
	end
	for _, m in L.marks do
		if m.x - m.r < 0 or m.y - m.r < 0 or m.x + m.r > w or m.y + m.r > h then
			clipped += 1
		end
		local wants = m.kind == "chat" and not (v.quiet and not ACTIVE[m.state])
		if wants and not m.named then
			unnamed += 1
		end
	end
	return {
		cover = near / total,
		empty = worst / math.min(w, h),
		nn_cv = cv,
		shape = math.abs(math.log((math.max(1, L.bw) / math.max(1, L.bh)) / (w / h))),
		overlap = over,
		over_marks = over_marks,
		clipped = clipped,
		unnamed = unnamed,
		marks = #L.marks,
		names = #L.names,
		zoom = L.zoom,
		steps = L.steps,
	}
end

local function json(v: any): string
	local t = typeof(v)
	if t == "number" then
		return if v ~= v or v == math.huge or v == -math.huge then "null" else string.format("%.4g", v)
	elseif t == "string" then
		return '"' .. string.gsub(v, '[%c"\\]', function(c: string): string
			return string.format("\\u%04x", string.byte(c))
		end) .. '"'
	elseif t == "boolean" then
		return tostring(v)
	elseif t == "table" then
		if #v > 0 or next(v) == nil then
			local parts = {}
			for _, x in v do
				table.insert(parts, json(x))
			end
			return "[" .. table.concat(parts, ",") .. "]"
		end
		local parts = {}
		for k, x in v do
			table.insert(parts, json(tostring(k)) .. ":" .. json(x))
		end
		return "{" .. table.concat(parts, ",") .. "}"
	end
	return "null"
end


local VARIANTS: { Variant } = {
	{ name = "K3", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true, even = true, even_share = 0.9 },
	{ name = "K3+sp06", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true, even = true, even_share = 0.9, spoke = 0.6 },
	{ name = "K3+sp08", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true, even = true, even_share = 0.9, spoke = 0.8 },
	{ name = "K2", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true, even = true },
	{ name = "K2+s18", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true, even = true, stretch_most = 1.8 },
	{ name = "K2+e09", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true, even = true, even_share = 0.9 },
	{ name = "K2+e10", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true, even = true, even_share = 1.0 },
	{ name = "K2+s18+e09", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true, even = true, stretch_most = 1.8, even_share = 0.9 },
	{ name = "K+even", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true, even = true },
	{ name = "K", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true },
	{ name = "K+turn22", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true, turn_below = 2.2 },
	{ name = "K+s16", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true, stretch_most = 1.6 },
	{ name = "K+turn22+s16", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true, turn_below = 2.2, stretch_most = 1.6 },
	{ name = "K+fold6", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true, fold = 6 },
	{ name = "K+fold10", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true, fold = 10 },
	{ name = "K+fold14", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true, fold = 14 },
	{ name = "W+T+Q+fit+fill", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true },
	{ name = "W+T+Q+fit+fill+turn", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, fill = true, turn = true },
	{ name = "W+T+F+Q+fit+fill+turn", wide = true, space = true, tight = true, fold = 4, quiet = true, min_fit = 0.5, fill = true, turn = true },
	{ name = "W+T+Q+fit+flat+turn", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5, flat = true, turn = true },
	{ name = "base" },
	{ name = "wide", wide = true, space = true },
	{ name = "tight", tight = true },
	{ name = "wide+tight", wide = true, space = true, tight = true },
	{ name = "fold", fold = 4 },
	{ name = "quiet", quiet = true },
	{ name = "fold+quiet", fold = 4, quiet = true },
	{ name = "wide+tight+fold+quiet", wide = true, space = true, tight = true, fold = 4, quiet = true },
	{ name = "wide+tight+fold+quiet+fit", wide = true, space = true, tight = true, fold = 4, quiet = true, min_fit = 0.5 },
	{ name = "wide+tight+quiet+fit", wide = true, space = true, tight = true, quiet = true, min_fit = 0.5 },
}

-- (The lab: every variant, shape and frame. Run on purpose:
-- FORCEBAL_ONLY=<variant names, comma-separated> narrows it.)
-- (The variants above: set ONLY to their names, comma-separated, to
-- measure them all again; LAYOUTS to print each layout for pictures.)
local ONLY = "base,K3"
local LAYOUTS = false
test("force balance: the kept layout (K3) against base, every shape and field: no name over another, none cut, spacing more even", function()
	local sum: { [string]: number } = {}
	local n: { [string]: number } = {}
	for _, v in VARIANTS do
		if not string.find("," .. ONLY .. ",", "," .. v.name .. ",", 1, true) then
			continue
		end
		for _, sname in SHAPE_ORDER do
			for _, f in FRAMES do
				local L = lay(SHAPES[sname], v, f[1], f[2])
				local m = measure_layout(L, v)
				m.variant, m.shape_name, m.frame = v.name, sname, `{f[1]}x{f[2]}`
				print(`zeron-zgx: FORCEBAL {json(m)}`)
				if LAYOUTS then
					print(`zeron-zgx: FORCEBALL {json({ variant = v.name, shape = sname, frame = m.frame, layout = L })}`)
				end
				sum[v.name] = (sum[v.name] or 0) + m.nn_cv
				n[v.name] = (n[v.name] or 0) + 1
				if v.name == "K3" then
					local tag = `{sname} {f[1]}x{f[2]}`
					expect(`{tag} overlap {m.overlap}`).to_be(`{tag} overlap 0`)
					expect(`{tag} clipped {m.clipped}`).to_be(`{tag} clipped 0`)
					expect(m.unnamed <= 2).to_be(true)
				end
			end
		end
	end
	local kept, base = sum.K3 / n.K3, sum.base / n.base
	print(`zeron-zgx: FORCEBAL_MEAN nn_cv base {string.format("%.2f", base)} kept {string.format("%.2f", kept)}`)
	expect(kept < base * 0.7).to_be(true)
end)

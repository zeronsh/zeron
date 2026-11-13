-- Where the Map (ds/orch_map) puts everything: lanes, stations, pills and
-- the lines between them, for a world and a width. Pure, so it can be
-- worked out and checked apart from drawing.
--
--   orch_map_layout.layout(world, width)  { h, lanes, nodes, pills, more,
--                                           edges, anchors, routes }

local M = {}

-- Geometry (points): each lane's head, the station column, a track's
-- height, the pills.
M.LANE_HEAD = 30
M.ORCH_W = 150
M.NODE_H = 28
M.TRACK_H = 42
M.LANE_PAD = 8
M.PILL_H = 30
local LANE_HEAD, ORCH_W, NODE_H, TRACK_H, LANE_PAD, PILL_H = M.LANE_HEAD, M.ORCH_W, M.NODE_H, M.TRACK_H, M.LANE_PAD, M.PILL_H
local GUTTER, PAD = 0, 0
local PILL_MAX, PILL_MIN = 184, 120
local PILL_GAP = 12
local SPINE_STEP = 8
local CORNER = 8

export type Pill = { id: string, lane: string, x: number, y: number, w: number, orchestrator: string, hue: number? }
export type Node_ = { id: string, lane: string, x: number, y: number, hue: number?, paused: boolean }
export type Lane = { id: string, name: string, y: number, h: number, online: boolean, here: boolean }
export type Edge = { orchestrator: string, hue: number?, paused: boolean, points: { number }, gaps: { { number } } }
export type Layout = {
	h: number,
	lanes: { Lane },
	nodes: { Node_ },
	pills: { Pill },
	more: { { id: string, x: number, y: number, n: number } },
	edges: { Edge },
	-- Where a pulse starts or ends for a chat id: { x, y }.
	anchors: { [string]: { number } },
	-- The line from a crew pill to its station, as points (x, y, …).
	routes: { [string]: { number } },
}


local function items_of(now: any): { [string]: any }
	local out = {}
	for _, it in (now and now.items) or {} do
		out[it.id] = it
	end
	return out
end

local function device_order(devices: { any }, here: string?): { any }
	local list = table.clone(devices or {})
	table.sort(list, function(a: any, b: any): boolean
		local ah, bh = a.id == here or a.here, b.id == here or b.here
		if ah ~= bh then
			return ah == true
		end
		local ac, bc = a.cloud == true or a.platform == "cloud", b.cloud == true or b.platform == "cloud"
		if ac ~= bc then
			return bc
		end
		return (a.name or "") < (b.name or "")
	end)
	return list
end

-- Where everything sits: lanes, stations, pills and the lines between
-- them, for a world and a width. Pure.
function M.layout(world: any, width: number): Layout
	local now = world.now or {}
	local chats = world.chats or {}
	local here = world.here
	local out: Layout = { h = 0, lanes = {}, nodes = {}, pills = {}, more = {}, edges = {}, anchors = {}, routes = {} }

	-- The orchestrators shown, in their order; each one's home and crew.
	local orchs = {}
	for _, o in now.orchestrators or {} do
		if not o.archived then
			table.insert(orchs, o)
		end
	end
	local crew_of: { [string]: { any } } = {}
	for _, m in now.crew or {} do
		if not m.released then
			crew_of[m.orchestrator_id] = crew_of[m.orchestrator_id] or {}
			table.insert(crew_of[m.orchestrator_id], m)
		end
	end
	local items = items_of(now)
	local function home_of(o: any): string?
		local c = chats[o.id]
		return (c and c.device) or here
	end

	-- The lanes: devices online, and any offline one a thread still runs on.
	local used: { [string]: boolean } = {}
	for _, o in orchs do
		local h = home_of(o)
		if h then
			used[h] = true
		end
		for _, m in crew_of[o.id] or {} do
			local c = chats[m.chat_id]
			if c and c.device then
				used[c.device] = true
			end
		end
	end
	local lanes: { any } = {}
	for _, d in device_order(world.devices or {}, here) do
		local online = d.online ~= false or d.id == here or d.here == true
		if online or used[d.id] then
			table.insert(lanes, { id = d.id, name = d.name or "", online = online, here = d.id == here or d.here == true })
		end
	end

	-- Which orchestrators drop a line to another lane: each takes a spine.
	local spine: { [string]: number } = {}
	local spines = 0
	for _, o in orchs do
		local h = home_of(o)
		for _, m in crew_of[o.id] or {} do
			local c = chats[m.chat_id]
			if c and c.device and c.device ~= h and not spine[o.id] then
				spines += 1
				spine[o.id] = spines
			end
		end
	end
	local spine_x0 = GUTTER + PAD + ORCH_W + 12
	local pills_x = spine_x0 + spines * SPINE_STEP + 14
	local room = math.max(PILL_MIN, width - pills_x)

	-- Each orchestrator's track in each lane: { y, pills }.
	local track_y: { [string]: { [string]: number } } = {}
	local y = 0
	for _, lane in lanes do
		local tracks = {}
		for _, o in orchs do
			local home = home_of(o) == lane.id
			local members = {}
			for _, m in crew_of[o.id] or {} do
				local c = chats[m.chat_id]
				local dev = (c and c.device) or home_of(o)
				if dev == lane.id then
					table.insert(members, m)
				end
			end
			if home or #members > 0 then
				table.sort(members, function(a: any, b: any): boolean
					local ia, ib = items[a.item_id or ""], items[b.item_id or ""]
					local oa, ob = (ia and ia.order) or 0, (ib and ib.order) or 0
					if oa ~= ob then
						return oa < ob
					end
					return a.chat_id < b.chat_id
				end)
				table.insert(tracks, { o = o, home = home, members = members })
			end
		end
		local h = LANE_HEAD + math.max(1, #tracks) * TRACK_H + LANE_PAD
		table.insert(out.lanes, { id = lane.id, name = lane.name, y = y, h = h, online = lane.online, here = lane.here })
		for ti, t in tracks do
			local ty = y + LANE_HEAD + (ti - 0.5) * TRACK_H
			track_y[t.o.id] = track_y[t.o.id] or {}
			track_y[t.o.id][lane.id] = ty
			local o = t.o
			if t.home then
				table.insert(out.nodes, { id = o.id, lane = lane.id, x = GUTTER + PAD, y = ty - NODE_H / 2, hue = o.hue, paused = o.paused == true })
				out.anchors[o.id] = { GUTTER + PAD + 9, ty }
			end
			-- The pills that fit, evenly wide; the rest a count.
			local n = #t.members
			local w = PILL_MAX
			local shown = n
			if n > 0 then
				w = math.clamp((room - (n - 1) * PILL_GAP) / n, PILL_MIN, PILL_MAX)
				local fit = math.max(1, math.floor((room + PILL_GAP) / (w + PILL_GAP)))
				if fit < n then
					shown = math.max(1, fit - 1)
					w = PILL_MIN
				end
			end
			for i = 1, shown do
				local m = t.members[i]
				local px = pills_x + (i - 1) * (w + PILL_GAP)
				table.insert(out.pills, { id = m.chat_id, lane = lane.id, x = px, y = ty - PILL_H / 2, w = w, orchestrator = o.id, hue = o.hue })
				out.anchors[m.chat_id] = { px + 10, ty }
			end
			if shown < n then
				table.insert(out.more, { id = `{o.id}@{lane.id}`, x = pills_x + shown * (w + PILL_GAP), y = ty - PILL_H / 2, n = n - shown })
			end
		end
		y += h
	end
	out.h = y

	-- The lines: each track from its station (or its spine) through its
	-- pills, gaps left where the pills stand.
	for _, o in orchs do
		local ys = track_y[o.id]
		local home = home_of(o)
		local hy = ys and home and ys[home]
		if ys and hy then
			local start = GUTTER + PAD + ORCH_W
			local sx = if spine[o.id] then spine_x0 + (spine[o.id] - 1) * SPINE_STEP else nil
			for lane_id, ty in ys do
				local pills = {}
				for _, p in out.pills do
					if p.orchestrator == o.id and p.lane == lane_id then
						table.insert(pills, p)
					end
				end
				local last = start
				for _, p in pills do
					last = math.max(last, p.x + p.w)
				end
				for _, mo in out.more do
					if mo.id == `{o.id}@{lane_id}` then
						last = math.max(last, mo.x + 36)
					end
				end
				local gaps = {}
				for _, p in pills do
					table.insert(gaps, { p.x - 3, p.x + p.w + 3 })
				end
				local points
				if lane_id == home then
					if #pills == 0 and not sx then
						continue
					end
					points = { start, ty, math.max(last, sx or start), ty }
				elseif sx then
					points = { start, hy, sx - CORNER, hy, sx, hy, sx, ty, sx + CORNER, ty, last, ty }
				else
					continue
				end
				table.insert(out.edges, { orchestrator = o.id, hue = o.hue :: number?, paused = o.paused == true, points = points, gaps = gaps })
				-- Each pill's way back to its station, for a pulse.
				for _, p in pills do
					local px, py = p.x + 10, ty
					if lane_id == home then
						out.routes[p.id] = { px, py, GUTTER + PAD + 9, hy }
					else
						out.routes[p.id] = { px, py, sx :: number, ty, sx :: number, hy, GUTTER + PAD + 9, hy }
					end
				end
			end
		end
	end
	return out
end

return M

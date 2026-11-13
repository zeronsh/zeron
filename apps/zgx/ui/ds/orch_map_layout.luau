-- Where the Map (ds/orch_map) puts everything: lanes, stations, pills and
-- the lines between them, for a world and a width. Pure, so it can be
-- worked out and checked apart from drawing.
--
--   orch_map_layout.layout(world, width)  { h, lanes, nodes, pills, more,
--                                           edges, anchors, routes }

local M = {}

-- Geometry (points): each lane's head, the station column, the pills.
M.LANE_HEAD = 30
M.ORCH_W = 150
M.NODE_H = 28
M.LANE_PAD = 8
M.PILL_H = 30
local LANE_HEAD, ORCH_W, NODE_H, LANE_PAD, PILL_H = M.LANE_HEAD, M.ORCH_W, M.NODE_H, M.LANE_PAD, M.PILL_H
local GUTTER, PAD = 0, 0
local PILL_MAX, PILL_MIN = 380, 160
-- A crew row's height, the most rows a block shows (the rest a count),
-- room between two orchestrators' blocks, and how far left of the pills
-- the trunk runs.
local ROW_H = 38
local MAX_ROWS = 5
local BLOCK_GAP = 6
local TRUNK_IN = 18
local SPINE_STEP = 8
local CORNER = 8

export type Pill = { id: string, lane: string, x: number, y: number, w: number, orchestrator: string, hue: number? }
export type Node_ = { id: string, lane: string, x: number, y: number, hue: number?, paused: boolean }
export type Lane = { id: string, name: string, y: number, h: number, online: boolean, here: boolean }
-- An orchestrator's lines: polylines (x, y, …), drawn as one shape with
-- round corners.
export type Edge = { orchestrator: string, hue: number?, paused: boolean, lines: { { number } } }
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
	local pills_x = spine_x0 + spines * SPINE_STEP + 14 + TRUNK_IN

	-- Each orchestrator's block in each lane: its station (home) or where
	-- its line arrives (another lane), and its crew under it, one row each,
	-- a trunk down beside them with an arm to each.
	local trunk_x = pills_x - TRUNK_IN
	local pill_w = math.clamp(width - pills_x, PILL_MIN, PILL_MAX)
	-- Per orchestrator, per lane: { top, rows = { y } }.
	local blocks: { [string]: { [string]: { top: number, rows: { number } } } } = {}
	local y = 0
	for _, lane in lanes do
		local h = LANE_HEAD
		local any = false
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
				any = true
				table.sort(members, function(a: any, b: any): boolean
					local ia, ib = items[a.item_id or ""], items[b.item_id or ""]
					local oa, ob = (ia and ia.order) or 0, (ib and ib.order) or 0
					if oa ~= ob then
						return oa < ob
					end
					return a.chat_id < b.chat_id
				end)
				local top = y + h
				local ty = top + ROW_H / 2
				if home then
					table.insert(out.nodes, { id = o.id, lane = lane.id, x = GUTTER + PAD, y = ty - NODE_H / 2, hue = o.hue :: number?, paused = o.paused == true })
					out.anchors[o.id] = { GUTTER + PAD + 9, ty }
				end
				local n = #members
				local shown = if n > MAX_ROWS then MAX_ROWS - 1 else n
				local rows = {}
				for i = 1, shown do
					local m = members[i]
					local ry = top + (i - 0.5) * ROW_H
					table.insert(rows, ry)
					table.insert(out.pills, { id = m.chat_id, lane = lane.id, x = pills_x, y = ry - PILL_H / 2, w = pill_w, orchestrator = o.id, hue = o.hue :: number? })
					out.anchors[m.chat_id] = { pills_x + 10, ry }
				end
				if shown < n then
					local ry = top + (shown + 0.5) * ROW_H
					table.insert(rows, ry)
					table.insert(out.more, { id = `{o.id}@{lane.id}`, x = pills_x, y = ry - PILL_H / 2, n = n - shown })
				end
				blocks[o.id] = blocks[o.id] or {}
				blocks[o.id][lane.id] = { top = top, rows = rows }
				h += math.max(1, #rows) * ROW_H + BLOCK_GAP
			end
		end
		if not any then
			h += ROW_H
		end
		h += LANE_PAD
		table.insert(out.lanes, { id = lane.id, name = lane.name, y = y, h = h, online = lane.online, here = lane.here })
		y += h
	end
	out.h = y

	-- The lines, one shape for each orchestrator: from its station out to
	-- its trunk, down past each of its crew with an arm to each (they're
	-- parallel work, not a sequence), and for crew on another machine, along
	-- its spine to that lane and the same there.
	local station_x = GUTTER + PAD + ORCH_W
	for _, o in orchs do
		local mine = blocks[o.id]
		local home = home_of(o)
		local hb = mine and home and mine[home]
		if not (mine and hb) then
			continue
		end
		local hy = hb.top + ROW_H / 2
		local sx = if spine[o.id] then spine_x0 + (spine[o.id] - 1) * SPINE_STEP else nil
		local lines: { { number } } = {}
		local function fan(arrive_y: number, rows: { number }, from_x: number)
			if #rows == 0 then
				return
			end
			-- In to the trunk at the first row, down it to the last.
			table.insert(lines, { from_x, arrive_y, trunk_x, arrive_y, trunk_x, rows[#rows] })
			for i, ry in rows do
				if i == 1 and math.abs(ry - arrive_y) < 0.5 then
					table.insert(lines, { trunk_x, ry, pills_x - 2, ry })
				else
					table.insert(lines, { trunk_x, ry - ROW_H / 2, trunk_x, ry, pills_x - 2, ry })
				end
			end
		end
		fan(hy, hb.rows, station_x)
		for lane_id, blk in mine do
			if lane_id ~= home and sx then
				local ay = blk.top + ROW_H / 2
				table.insert(lines, { station_x, hy, sx, hy, sx, ay, sx + CORNER * 2, ay })
				fan(ay, blk.rows, sx + CORNER * 2)
			end
		end
		if #lines > 0 then
			table.insert(out.edges, { orchestrator = o.id, hue = o.hue :: number?, paused = o.paused == true, lines = lines })
		end
		-- Each pill's way back to its station, for a pulse.
		for lane_id, blk in mine do
			for _, p in out.pills do
				if p.orchestrator == o.id and p.lane == lane_id then
					local py = p.y + PILL_H / 2
					local ay = blk.top + ROW_H / 2
					if lane_id == home then
						out.routes[p.id] = { p.x + 10, py, trunk_x, py, trunk_x, hy, GUTTER + PAD + 9, hy }
					elseif sx then
						out.routes[p.id] = { p.x + 10, py, trunk_x, py, trunk_x, ay, sx :: number, ay, sx :: number, hy, GUTTER + PAD + 9, hy }
					end
				end
			end
		end
	end
	return out
end

return M

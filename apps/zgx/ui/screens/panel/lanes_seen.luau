--!strict
-- Which lanes (side chats, subagents) you've seen since they last
-- finished, as the sidebar's sessions have it: a lane done or failed shows
-- its glyph until you open its tab (from the panel home, a spawn chip, the
-- lanes card: anywhere a tab of it comes to the front), then its time. One
-- that finishes again (a side chat's new reply) shows it again: its finish
-- is later than when you last saw it.
--
--   seen.unseen(lane)   whether a lane `{ id, status?, at? }` done or failed
--                       hasn't been seen since (tracked)
--   seen.mark(id)       seen now
--
-- Kept across launches, the latest 500 lanes (the oldest seen forgotten
-- first: a lane that old shows its glyph again, which is the safe side).

local state = require("@app/screens/panel/state")

local M = {}

local CAP = 500
-- Lane id → when it was last seen (unix seconds).
M.store = ui.persist("panel_lanes_seen", {} :: { [string]: number })

function M.mark(id: string, at: number?)
	local now = at or os.time()
	local all = M.store:peek()
	if all[id] == now then
		return
	end
	local next = table.clone(all)
	next[id] = now
	-- Over the cap: the longest ago go.
	local n = 0
	for _ in next do
		n += 1
	end
	if n > CAP then
		local order: { { id: string, at: number } } = {}
		for k, v in next do
			table.insert(order, { id = k, at = v })
		end
		table.sort(order, function(a, b): boolean
			return a.at < b.at
		end)
		for i = 1, n - CAP do
			next[order[i].id] = nil
		end
	end
	M.store:set(next)
end

-- Done or failed, and not seen since it last moved.
function M.unseen(lane: { id: string, status: string?, at: number? }): boolean
	local k = lane.status
	if k ~= "done" and k ~= "failed" then
		return false
	end
	local at = M.store:get()[lane.id]
	return at == nil or at < (lane.at or 0)
end

-- A lane's tab coming to the front, in any session's panel, is seeing it;
-- so is leaving it (what it said while it was in front was seen).
local front: { [string]: boolean } = {}
ui.effect(function()
	local now: { [string]: boolean } = {}
	for _, s in state.sessions:get() do
		local active = s.active
		if active then
			for _, t in s.tabs do
				if t.id == active and (t.kind == "side_chat" or t.kind == "subagent") and t.chat then
					now[t.chat] = true
				end
			end
		end
	end
	for id in now do
		if not front[id] then
			M.mark(id)
		end
	end
	for id in front do
		if not now[id] then
			M.mark(id)
		end
	end
	front = now
end)

return M

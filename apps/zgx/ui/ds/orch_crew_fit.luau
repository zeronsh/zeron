--!strict
-- How many of an orchestrator's working crew threads its side panel's Crew
-- tab shows (ds/orch_crew_list): never fewer than MIN; past that, as many as
-- the panel has room for, up to MAX; only what doesn't fit folds into "n
-- more". A row is a few dozen nodes, each kept live: MAX bounds what one
-- panel builds and keeps current, the rest one "n more" away.
--
--   Fit.next(shown, total, room, row_h) -> number
--     shown   rows shown now
--     total   working threads
--     room    height left free under the list in its scroll (< 0: overflows)
--     row_h   a row's height as laid out (0: not yet)

local M = {}

M.MIN = 10
M.MAX = 40

-- The rows to show next: moved by the room, a row's height at a time.
function M.next(shown: number, total: number, room: number, row_h: number): number
	if row_h <= 0 then
		return shown
	end
	local want = shown
	if room < 0 then
		want = shown - math.ceil(-room / row_h)
	elseif room >= row_h and shown < total then
		want = shown + math.floor(room / row_h)
	end
	return math.clamp(math.min(want, total), M.MIN, M.MAX)
end

return M

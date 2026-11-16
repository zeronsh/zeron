-- Run counts for the Control room's hot spots (tests/orch_perf.test.luau):
-- how many times a Board card's status line and a Progress unit's activity
-- line were worked out. Plain counters, never state: counting reruns
-- nothing.
--
--   local trace = require("@ds/orch_perf_trace")
--   trace.hit("board.status")
--   trace.counts   -- { [name] = n }

local M = {}

M.counts = {} :: { [string]: number }

function M.hit(name: string)
	M.counts[name] = (M.counts[name] or 0) + 1
end

function M.reset()
	M.counts = {}
end

return M

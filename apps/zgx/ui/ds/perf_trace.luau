-- Run counts for the app's hot spots (tests/whole_list.test.luau): how many
-- times a sidebar row took a new session. Plain counters, never state:
-- counting reruns nothing.
--
--   local trace = require("@ds/perf_trace")
--   trace.hit("sidebar.row")
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

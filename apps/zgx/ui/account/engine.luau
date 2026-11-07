-- The engine, as the host keeps it (src/engine/mod.rs): whether it can't
-- start (an engine this app can start that hasn't answered in a minute),
-- and Retry, which starts or reaches it again at once and goes back to
-- waiting. An engine that answers meanwhile clears it by itself.
--
--   engine.failed:get()   it can't start
--   engine.retry()        tries again

local M = {}

M.start = ui.shared("engine.start", { failed = false })

M.failed = ui.computed(function(): boolean
	return (M.start:get() :: any).failed == true
end)

function M.retry()
	ui.invoke("engine.retry")
end

return M

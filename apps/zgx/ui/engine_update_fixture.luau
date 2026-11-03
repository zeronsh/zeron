-- The engine-update chip (screens/engine/update) over a made-up host:
-- `engine.update` as the test sets it, the restart noted in
-- `test_engine_restarts`, the sessions' chats as `test_engine_chats` says.
-- Driven by tests/engine_update.test.luau.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Update = require("@app/screens/engine/update")
local sessions = require("@account/sessions") :: any

local restarts = ui.state("test_engine_restarts", 0)
Update.call_restart = function(): any
	restarts:set(restarts:peek() + 1)
	-- (A promise that never settles: the engine goes, and comes back new.)
	return { after = function() end }
end

-- The chats, as the engine's sessions would have them.
local chats = ui.state("test_engine_chats", {} :: { any })
ui.effect(function()
	sessions.now:set({ state = "ready", chats = chats:get(), archived = {}, devices = {}, projects = {} })
end)

return ui.column {
	w = 240,
	p = "$space.2",
	Update.Watch(),
	Update.Notice(),
}

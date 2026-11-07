-- Setup's window with the engine as the host would have it (@account/engine):
-- for tests/engine_start.test.luau. The account restoring, as at a launch.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local auth = require("@account/auth")
auth.account:set({ state = "restoring", env = "staging" } :: any)
local engine = require("@account/engine") :: any
local retries = ui.state("fixture_engine_retries", 0)
engine.retry = function()
	retries:set(retries:peek() + 1)
	engine.start:set({ failed = false })
end
ui.command {
	id = "fixture.engine_cant_start",
	title = "Fixture: the engine hasn't started in a minute",
	run = function()
		engine.start:set({ failed = true })
	end,
}
ui.command {
	id = "fixture.engine_started",
	title = "Fixture: the engine answers",
	run = function()
		engine.start:set({ failed = false })
	end,
}
return require("@app/screens/onboarding")()

-- The Control room as a first real run leaves it (ui/control_fixture.luau,
-- its board's work replaced): one orchestrator, Tally, and the two things
-- it finished today, the rest of the board empty. For the board's audit
-- pictures (`control_board_live@1440x1400|#control-widget-board`).

local built = require("./control_fixture")
local orchestration = require("@account/orchestration")

local now = os.time()
local n = table.clone(orchestration.now:peek())
n.orchestrators = {
	{ id = "orc-tally", kind = "standard", name = "Tally", hue = 214, goal = "Line counts that never lie.", rules = "", crew_rules = "", notes = "", crew = { devices = {} }, paused = false, archived = false },
}
n.items = {
	{
		id = "it-tests",
		orchestrator_id = "orc-tally",
		title = "Add pytest tests for count()",
		state = "done",
		priority = "normal",
		order = 1,
		outcome = "tests/test_tally.py with 11 passing pytest tests for count(), committed as 9bf0111.",
		created = now - 3600,
		updated = now - 600,
		done = now - 600,
	},
	{
		id = "it-lines",
		orchestrator_id = "orc-tally",
		title = "Add --lines flag to tally.py",
		state = "done",
		priority = "normal",
		order = 2,
		outcome = "--lines flag added to tally.py, committed as 117123a. Verified: word count 3, --lines 2.",
		created = now - 3000,
		updated = now - 300,
		done = now - 300,
	},
}
orchestration.now:set(n)

return built

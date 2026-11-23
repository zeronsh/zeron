-- The Control room as a first real run leaves it (ui/control_page_fixture.luau,
-- its board's work replaced): one orchestrator, Tally, and the two things
-- it finished today, the rest of the board empty. For the board's audit
-- pictures (`control_board_live@1440x2600|#control-board-tab`) and
-- tests/orch_board.test.luau.

local built = require("./control_page_fixture")
local orchestration = require("@orchestrators/account/orchestration")
local sessions = require("@account/sessions")

local now = os.time()
-- (Done today whatever the clock says: never before this midnight.)
local here = os.date("*t", now) :: any
local midnight = now - (here.hour * 3600 + here.min * 60 + here.sec)
local function today(ago: number): number
	return math.max(midnight, now - ago)
end
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
		updated = today(600),
		done = today(600),
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
		updated = today(300),
		done = today(300),
	},
}
orchestration.now:set(n)
-- (Tally's chat, in its project: named as Tally is, so its cards say Tally
-- alone.)
local s = table.clone(sessions.now:peek())
s.projects = table.clone(s.projects or {})
table.insert(s.projects, { id = "p-tally", name = "tally", device = "dev-cloud", path = "/work/tally", git = true })
s.chats = table.clone(s.chats or {})
table.insert(s.chats, { id = "orc-tally", title = "Tally", device = "dev-cloud", project = "p-tally", updated = now - 300, created = now - 9000, agent = "claude", model = "claude-opus-5-5" })
sessions.now:set(s)
-- (On the Board's own tab.)
require("@orchestrators/screens/control/page").tab:set("board")

return built

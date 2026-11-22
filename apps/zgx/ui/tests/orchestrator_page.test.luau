-- An orchestrator's page (screens/orchestrator/page) in the sample world
-- (ui/orchestrator_fixture.luau): its head, its conversation with the
-- crew's reports as one Crew updates card, its side panel (what needs you
-- and its board, its crew: tests/orch_side_panel), split wide and one at a
-- time narrow; and the New
-- orchestrator dialog and its settings, each asking the one op.

local function settle(app: any)
	app:frames(30)
end

local function open(w: number?, h: number?): any
	local app = mount("orchestrator_fixture.luau")
	app:resize(w or 1440, h or 900)
	settle(app)
	return app
end

-- Whether a node shows (there, and not hidden).
local function shown(app: any, sel: string): boolean
	if not app:has(sel) then
		return false
	end
	local first = string.match(app:tree(sel), "[^\n]+") or ""
	return string.find(first, " hidden", 1, true) == nil
end

-- The ops asked, one a line.
local function calls(app: any): string
	return app:state("orch_calls") :: string
end

test("orchestrator page: its head says what it is and how its crew stands", function()
	local app = open()
	expect(app:has('#orch-name >> "Orchestrators"')).to_be(true)
	expect(app:has('#orch-goal >> "Ship Orchestrators and the Control room in zgx."')).to_be(true)
	-- Three of its four crew threads work now.
	expect(app:has('#orch-crew-count >> "3 of 4 working"')).to_be(true)
	expect(app:has('#orch-model >> "Opus 5.5"')).to_be(true)
end)

test("orchestrator page: wide, its conversation and live state side by side", function()
	local app = open(1440, 900)
	expect(shown(app, "#orch-conversation")).to_be(true)
	expect(shown(app, "#orch-side")).to_be(true)
	-- The crew's reports are one card in its conversation, never a bubble.
	expect(app:has('#orch-conversation >> "Crew updates"')).to_be(true)
	expect(app:has('#orch-conversation >> "Needs a decision: tabs or split for the orchestrator page"')).to_be(true)
	-- Its side panel: what needs it, its board (its own items only), its crew.
	expect(app:has('#orch-section-needs >> "Orchestrator page: conversation and live state side by side, or tabs?"')).to_be(true)
	expect(app:wait('#orch-page-board-now >> "Control room"', { timeout_ms = 2000 })).to_be(true)
	expect(app:has('#orch-page-board >> "Reboot Moscow node"')).to_be(false)
	expect(app:has('#orch-panel-crew >> "Engine: orchestration state"')).to_be(true)
end)

test("orchestrator page: narrow, one half at a time, switched in its head", function()
	local app = open(900, 800)
	-- Split with no room for both: the conversation.
	expect(shown(app, "#orch-conversation")).to_be(true)
	expect(shown(app, "#orch-side")).to_be(false)
	app:click('#orch-head >> role:radio "Live"')
	settle(app)
	expect(shown(app, "#orch-side")).to_be(true)
	expect(shown(app, "#orch-conversation")).to_be(false)
	-- Split, widened: both.
	app:click('#orch-head >> role:radio "Split"')
	app:resize(1600, 900)
	settle(app)
	expect(shown(app, "#orch-conversation")).to_be(true)
	expect(shown(app, "#orch-side")).to_be(true)
	app:resize(900, 700)
	settle(app)
	expect(shown(app, "#orch-side")).to_be(false)
	expect(shown(app, "#orch-conversation")).to_be(true)
end)

test("orchestrator page: an ask's choice answers it", function()
	local app = open()
	app:click('#orch-section-needs >> role:button "Side by side"')
	settle(app)
	-- (Answered at once, sent once its Undo has passed: the option 1-based.)
	expect(app:wait('#orch-section-needs >> role:button "Undo"', { gone = true, timeout_ms = 8000 })).to_be(true)
	settle(app)
	expect(calls(app)).to_contain("answer_ask:id=ak-1,option=1")
end)

test("orchestrator page: pause, and its settings open on its fields", function()
	local app = open()
	app:click("#orch-pause")
	settle(app)
	expect(calls(app)).to_contain("updateOrchestrator:id=orc-orch,paused=true")
	app:click("#orch-settings")
	settle(app)
	expect(app:state("orch_settings_open")).to_be(true)
	expect(app:state("orch_set_goal")).to_be("Ship Orchestrators and the Control room in zgx.")
	expect(app:state("orch_set_limit")).to_be(4)
	-- A rule changed and saved: only what changed goes.
	app:set_state("orch_set_rules", "Opus for everything.")
	settle(app)
	app:click("#orch-settings-save")
	settle(app)
	local said = calls(app)
	expect(said).to_contain("updateOrchestrator:id=orc-orch,rules=Opus for everything.")
	expect(string.find(said, "goal=", 1, true)).to_be(nil)
end)

-- Rules, crew rules and notes say who reads them: a hint under each, and
-- an example where one is empty.
local HINTS = {
	"Only the orchestrator follows these. Its crew doesn't see them.",
	"Every crew thread gets these with its first message, and again when you change them.",
	"The orchestrator keeps its plan here and updates it as it works. Crew threads don't see it.",
}
local PLACEHOLDERS = {
	"Ask me before merging. Opus for UI, Sonnet for the rest.",
	"Own worktree per thread. Run the tests before reporting done.",
	"Empty until the orchestrator writes its plan",
}
local FIELDS = { "Orchestrator rules", "Crew rules", "Plan and notes" }

test("orchestrator settings: rules, crew rules and notes filled, each with its hint", function()
	local app = open()
	app:command("orch.settings")
	settle(app)
	expect(app:state("orch_settings_open")).to_be(true)
	for _, name in FIELDS do
		expect(app:has(`role:textbox "{name}"`)).to_be(true)
	end
	for _, hint in HINTS do
		expect(app:has(`"{hint}"`)).to_be(true)
	end
	expect(app:text('role:textbox "Orchestrator rules"')).to_contain("Ask before merging to zeron/zgx.")
	expect(app:text('role:textbox "Crew rules"')).to_contain("Own worktree per thread.")
	expect(app:text('role:textbox "Plan and notes"')).to_contain("1. Contract (done)")
	-- Filled: no examples over what's written.
	for _, example in PLACEHOLDERS do
		expect(app:has(`"{example}"`)).to_be(false)
	end
	expect(app:error()).to_be_nil()
end)

test("orchestrator settings: empty rules, crew rules and notes show an example", function()
	local app = open()
	app:command("orch.settings_empty")
	settle(app)
	expect(app:state("orch_settings_open")).to_be(true)
	expect(app:state("orch_set_rules")).to_be("")
	expect(app:state("orch_set_crew_rules")).to_be("")
	expect(app:state("orch_set_notes")).to_be("")
	for _, name in FIELDS do
		expect(app:has(`role:textbox "{name}"`)).to_be(true)
	end
	for _, example in PLACEHOLDERS do
		expect(app:has(`"{example}"`)).to_be(true)
	end
	for _, hint in HINTS do
		expect(app:has(`"{hint}"`)).to_be(true)
	end
	expect(app:error()).to_be_nil()
end)

test("orchestrator page: New orchestrator asks createOrchestrator and opens it", function()
	local app = open()
	app:set_state("orch_new_open", true)
	settle(app)
	expect(app:has("#orch-new-dialog")).to_be(true)
	app:set_state("orch_new_name", "Release train")
	app:set_state("orch_new_goal", "Ship 0.9 on Friday")
	settle(app)
	app:click('role:button "Create"')
	settle(app)
	expect(calls(app)).to_contain("createOrchestrator:goal=Ship 0.9 on Friday,name=Release train")
	expect(app:state("orch_new_open")).to_be(false)
	expect(app:state("orch_route")).to_be("orchestrator:orc-orch")
end)

test("orchestrator page: another orchestrator's page in place", function()
	local app = open()
	app:command("orch.vpn")
	settle(app)
	expect(app:has('#orch-name >> "Exnomic VPN"')).to_be(true)
	expect(app:has('#orch-page-board >> "Reboot Moscow node"')).to_be(true)
	app:command("orch.orch")
	settle(app)
	expect(app:has('#orch-name >> "Orchestrators"')).to_be(true)
end)

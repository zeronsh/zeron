-- The New orchestrator dialog (screens/orchestrator/dialogs) in the sample
-- world (ui/orchestrator_fixture.luau, whose ops answer at once; this
-- computer is "desk", the other "macbook"). Its fields in order, labelled;
-- Create waiting for a name and saying so; each field reaching
-- createOrchestrator; its defaults (this device's crew, 3 at once, the
-- crew on the same model, the optional part folded); then, created, the
-- new orchestrator's page with its composer taking the keys and one line
-- on what to tell it first. And the two it had before: leaving the name
-- field keeps it, and one submit asks once.

local NAME = '#orch-new-dialog >> role:textbox "Name"'
local GOAL = "#orch-new-dialog >> role:textbox \"What it's for\""
local CREATE = 'role:button "Create and open"'

local function settle(app: any)
	app:frames(30)
end

local function open(): any
	local app = mount("orchestrator_fixture.luau")
	app:resize(1440, 900)
	settle(app)
	app:set_state("orch_new_open", true)
	settle(app)
	return app
end

-- The createOrchestrator ops asked, one a line.
local function creates(app: any): { string }
	local out: { string } = {}
	for line in (app:state("orch_calls") :: string):gmatch("[^\n]+") do
		if line:find("createOrchestrator:", 1, true) == 1 then
			table.insert(out, line)
		end
	end
	return out
end

-- The last createOrchestrator op, whole.
local function sent(app: any): any
	local all = app:state("orch_creates") :: { any }
	return all[#all]
end

-- The first line of a node's tree.
local function line_of(app: any, sel: string): string
	return string.match(app:tree(sel), "[^\n]+") or ""
end

-- Whether a checkbox's line says it's checked (not "unchecked").
local function checked(app: any, sel: string): boolean
	return string.find(line_of(app, sel), " checked$") ~= nil or string.find(line_of(app, sel), " checked ", 1, true) ~= nil
end

-- What the dialog says, all of it.
local function words(app: any): string
	return app:tree("#orch-new-dialog") :: string
end

local function disabled(app: any, sel: string): boolean
	return string.find(line_of(app, sel), "disabled", 1, true) ~= nil
end

test("orch new: its fields in order, labelled, the optional part folded", function()
	local app = open()
	local tree = app:tree("#orch-new-dialog") :: string
	local at = function(word: string): number
		return (string.find(tree, word, 1, true)) or 0
	end
	-- Name, what it's for, project and model, where the crew runs, at once.
	expect(at('textbox "Name"') > 0).to_be(true)
	expect(at("What it's for") > at('textbox "Name"')).to_be(true)
	expect(at('button "Project"') > at("What it's for")).to_be(true)
	expect(at('button "Model"') > at('button "Project"')).to_be(true)
	expect(at("Crew runs on") > at('button "Model"')).to_be(true)
	expect(at("At once") > at("Crew runs on")).to_be(true)
	-- Its rules and flow wait under "optional".
	expect(app:has("#orch-new-more")).to_be(true)
	expect(app:has("#orch-new-optional")).to_be(false)
	app:click("#orch-new-more")
	settle(app)
	expect(app:has("#orch-new-optional")).to_be(true)
	expect(app:has('#orch-new-optional >> role:textbox "Orchestrator rules"')).to_be(true)
	expect(app:has('#orch-new-optional >> role:textbox "Crew rules"')).to_be(true)
	expect(app:has('#orch-new-optional >> role:button "Start from"')).to_be(true)
	expect(app:has('#orch-new-optional >> role:button "Import JSON"')).to_be(true)
	expect(app:state("orch_new_more")).to_be(true)
end)

test("orch new: its defaults are this device's crew, 3 at once, the same model", function()
	local app = open()
	-- This computer's chip ticked, the other not.
	expect(checked(app, "#orch-new-devices-dev-desk")).to_be(true)
	expect(checked(app, "#orch-new-devices-dev-mac")).to_be(false)
	-- (Cloud isn't a crew device yet.)
	expect(app:has("#orch-new-devices-dev-cloud")).to_be(false)
	expect(app:state("orch_new_limit")).to_be(3)
	expect(words(app)).to_contain("Its crew uses the same model.")
	expect(words(app)).to_contain("Runs on desk.")
	-- Unticking its only device leaves it there (none is its own).
	app:click("#orch-new-devices-dev-desk")
	settle(app)
	expect(checked(app, "#orch-new-devices-dev-desk")).to_be(true)
end)

test("orch new: Create waits for a name, and says so", function()
	local app = open()
	expect(disabled(app, CREATE)).to_be(true)
	expect(words(app)).to_contain("Give it a name to create it.")
	app:click(CREATE)
	app:key("Enter")
	settle(app)
	expect(#creates(app)).to_be(0)
	expect(app:state("orch_new_open")).to_be(true)
	-- Spaces alone aren't a name.
	app:type(NAME, "   ")
	settle(app)
	expect(disabled(app, CREATE)).to_be(true)
	app:key("Enter")
	settle(app)
	expect(#creates(app)).to_be(0)
	-- A name: Create takes it, and the line goes.
	app:type(NAME, "Release train")
	settle(app)
	expect(disabled(app, CREATE)).to_be(false)
	expect(string.find(words(app), "Give it a name", 1, true)).to_be(nil)
end)

test("orch new: every field reaches createOrchestrator", function()
	local app = open()
	app:type(NAME, "Release train")
	app:type(GOAL, "Ship 0.9 on Friday")
	app:set_state("orch_new_project", "zeron")
	app:set_state("orch_new_model", "claude-code/claude-opus-5-5")
	settle(app)
	-- The crew on the other computer too, 5 at once.
	app:click("#orch-new-devices-dev-mac")
	app:set_state("orch_new_limit", 5)
	app:click("#orch-new-more")
	settle(app)
	app:type('#orch-new-optional >> role:textbox "Orchestrator rules"', "Opus for planning.")
	app:type('#orch-new-optional >> role:textbox "Crew rules"', "Small commits.")
	app:set_state("orch_new_crew_model", "claude-code/claude-haiku-5-5")
	settle(app)
	expect(words(app)).to_contain("Its crew uses Haiku 5.5.")
	app:click(CREATE)
	settle(app)
	expect(#creates(app)).to_be(1)
	local op = sent(app)
	expect(op.name).to_be("Release train")
	expect(op.goal).to_be("Ship 0.9 on Friday")
	expect(op.spaceId).to_be("zeron")
	expect(op.harness).to_be("claude-code")
	expect(op.model).to_be("claude-opus-5-5")
	expect(op.rules).to_be("Opus for planning.")
	expect(op.crewRules).to_be("Small commits.")
	expect(table.concat(op.crew.devices, ",")).to_be("dev-desk,dev-mac")
	expect(op.crew.maxRunning).to_be(5)
	expect(op.crew.harness).to_be("claude-code")
	expect(op.crew.model).to_be("claude-haiku-5-5")
	expect(type(op.requestId)).to_be("string")
end)

test("orch new: left as they are, the crew is its own device's, 3 at once, its own model", function()
	local app = open()
	app:type(NAME, "Defaults")
	settle(app)
	app:click(CREATE)
	settle(app)
	local op = sent(app)
	-- (Its own device is the engine's empty list; the same model, none.)
	expect(op.crew.devices).to_be(nil)
	expect(op.crew.maxRunning).to_be(3)
	expect(op.crew.model).to_be(nil)
	expect(op.spaceId).to_be(nil)
	expect(op.deviceId).to_be(nil)
	expect(op.rules).to_be(nil)
end)

test("orch new: No limit is 0, and sends none", function()
	local app = open()
	app:set_state("orch_new_limit", 0)
	settle(app)
	expect(app:tree('#orch-new-dialog >> role:spinbutton "At once"')).to_contain("No limit")
	app:type(NAME, "Unbounded")
	app:click(CREATE)
	settle(app)
	expect(sent(app).crew.maxRunning).to_be(nil)
end)

test("orch new: no project on another computer puts it there", function()
	local app = open()
	app:set_state("orch_new_project", "device:dev-mac")
	settle(app)
	expect(words(app)).to_contain("Runs on macbook.")
	-- Its crew follows it there.
	expect(checked(app, "#orch-new-devices-dev-mac")).to_be(true)
	app:type(NAME, "On the mac")
	app:click(CREATE)
	settle(app)
	local op = sent(app)
	expect(op.deviceId).to_be("dev-mac")
	expect(op.spaceId).to_be(nil)
	expect(op.crew.devices).to_be(nil)
end)

test("orch new: created, its page opens with the composer taking the keys and a first line", function()
	local app = open()
	app:type(NAME, "Fresh start")
	settle(app)
	app:key("Enter")
	app:frames(60)
	expect(app:state("orch_new_open")).to_be(false)
	expect(app:state("orch_route")).to_be("orchestrator:orc-fresh")
	expect(app:has("#orch-first-hint")).to_be(true)
	expect(app:tree("#orch-first-hint")).to_contain("Tell it what to work on first")
	-- (Once the dialog has gone.)
	app:wait(400)
	app:frames(10)
	expect(app:tree('#orch-conversation >> role:textbox "Message"')).to_contain("focused")
	-- (Asked once: a later visit doesn't take the keys again.)
	expect(app:state("orch_just_made")).to_be(nil)
end)

-- Its two earlier regressions.

test("orch new: leaving the name field keeps the dialog and the name", function()
	local app = open()
	app:type(NAME, "Release")
	settle(app)
	-- (Tab: on to the next field.)
	app:key("Tab")
	settle(app)
	expect(app:state("orch_new_open")).to_be(true)
	expect(app:has("#orch-new-dialog")).to_be(true)
	expect(app:state("orch_new_name")).to_be("Release")
	expect(#creates(app)).to_be(0)
	-- And a click elsewhere in the dialog.
	app:type(NAME, " train")
	app:click(GOAL)
	settle(app)
	expect(app:state("orch_new_open")).to_be(true)
	expect(app:state("orch_new_name")).to_be("Release train")
	expect(#creates(app)).to_be(0)
end)

test("orch new: typing then clicking Create asks once", function()
	local app = open()
	app:type(NAME, "Release train")
	settle(app)
	app:click(CREATE)
	settle(app)
	local lines = creates(app)
	expect(#lines).to_be(1)
	expect(lines[1]).to_contain("name=Release train")
	expect(lines[1]).to_contain("requestId=")
	expect(app:state("orch_new_open")).to_be(false)
	expect(app:state("orch_route")).to_be("orchestrator:orc-orch")
end)

test("orch new: Enter, Enter again and a click ask once", function()
	local app = open()
	app:type(NAME, "Release train")
	settle(app)
	app:key("Enter")
	app:key("Enter")
	app:frames(2)
	if app:has(CREATE) then
		app:click(CREATE)
	end
	settle(app)
	expect(#creates(app)).to_be(1)
end)

test("orch new: each opening is its own request; sent, the draft is gone", function()
	local app = open()
	app:type(NAME, "One")
	app:key("Enter")
	settle(app)
	expect(app:state("orch_new_name")).to_be("")
	app:set_state("orch_new_open", true)
	settle(app)
	app:type(NAME, "Two")
	app:key("Enter")
	settle(app)
	local lines = creates(app)
	expect(#lines).to_be(2)
	local a = string.match(lines[1], "requestId=([^,]+)")
	local b = string.match(lines[2], "requestId=([^,]+)")
	expect(a ~= nil and b ~= nil and a ~= b).to_be(true)
end)

test("orch new: closed unsent, the draft is there again as it reopens", function()
	local app = open()
	app:type(NAME, "Half a name")
	settle(app)
	app:key("Escape")
	settle(app)
	expect(app:state("orch_new_open")).to_be(false)
	expect(#creates(app)).to_be(0)
	app:set_state("orch_new_open", true)
	settle(app)
	expect(app:state("orch_new_name")).to_be("Half a name")
	expect(app:text(NAME)).to_contain("Half a name")
end)

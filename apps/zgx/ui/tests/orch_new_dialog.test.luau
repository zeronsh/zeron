-- The New orchestrator dialog (screens/orchestrator/dialogs) in the sample
-- world (ui/orchestrator_fixture.luau, whose ops answer at once): leaving
-- its name field keeps it open with the name; one submit, however it comes
-- (Enter, Enter again, a click on Create after it), asks createOrchestrator
-- once; a draft closed unsent is there again as it reopens.

local NAME = '#orch-new-dialog >> role:textbox "Name"'

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
	app:click('#orch-new-dialog >> role:textbox "Goal"')
	settle(app)
	expect(app:state("orch_new_open")).to_be(true)
	expect(app:state("orch_new_name")).to_be("Release train")
	expect(#creates(app)).to_be(0)
end)

test("orch new: typing then clicking Create asks once", function()
	local app = open()
	app:type(NAME, "Release train")
	settle(app)
	app:click('role:button "Create"')
	settle(app)
	local sent = creates(app)
	expect(#sent).to_be(1)
	expect(sent[1]).to_contain("name=Release train")
	expect(sent[1]).to_contain("requestId=")
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
	if app:has('role:button "Create"') then
		app:click('role:button "Create"')
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
	local sent = creates(app)
	expect(#sent).to_be(2)
	local a = string.match(sent[1], "requestId=([^,]+)")
	local b = string.match(sent[2], "requestId=([^,]+)")
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

test("orch new: as Zeron's New dialogs, a line under its title, its name, its goal, then its chips", function()
	local app = open()
	expect(app:has('"An agent that plans the work and runs a crew to finish it."')).to_be(true)
	expect(app:has(NAME)).to_be(true)
	expect(app:has('#orch-new-dialog >> role:textbox "Goal"')).to_be(true)
	-- (The chips say what they hold, named for assistive technology, no
	-- label over them: the only text in the dialog besides the name's.)
	local tree = app:tree("#orch-new-dialog") :: string
	expect(app:tree('#orch-new-dialog >> role:button "Project"')).to_contain('"No project"')
	expect(app:tree('#orch-new-dialog >> role:button "Device"')).to_contain('"This device"')
	expect(app:tree('#orch-new-dialog >> role:button "Model"')).to_contain('"Default model"')
	expect(tree).to_contain('text "Name"')
	expect(string.find(tree, 'text "Model"', 1, true)).to_be(nil)
	expect(string.find(tree, 'text "Goal"', 1, true)).to_be(nil)
	-- (Its rules are its settings', not here.)
	expect(string.find(tree, "Rules", 1, true)).to_be(nil)
end)

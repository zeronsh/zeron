-- The orchestrator page's Flow view (ds/orch_flow_editor) in the sample
-- world (ui/orch_flow_fixture.luau): Work | Flow in its head; the flow's
-- blocks on the field and the one selected in the inspector; a block's
-- edits kept for the orchestrator; with no flow yet, its starters. And the
-- New orchestrator dialog's Start from: the flow chosen kept for the one
-- made.

local function settle(app: any)
	app:frames(30)
end

local function open(): any
	local app = mount("orch_flow_fixture.luau")
	app:resize(1440, 900)
	settle(app)
	return app
end

local function saved(app: any): any
	return (app:state("orch_flows") :: any)["orc-orch"]
end

test("orch flow page: Flow shows the field and the selected block", function()
	local app = open()
	expect(app:has("#orch-view")).to_be(true)
	expect(app:has("#orch-flow-field")).to_be(true)
	expect(app:has("#orch-flow-toolbar")).to_be(true)
	expect(app:has('#orch-flow-block >> "Worker"')).to_be(true)
	-- (The work itself is out of view, kept.)
	expect(app:has("#orch-flow-canvas")).to_be(true)
end)

test("orch flow page: a block's name, changed, is kept for the orchestrator", function()
	local app = open()
	app:type('#orch-flow-block >> role:textbox "Name"', " two")
	app:key("Enter")
	settle(app)
	local f = saved(app)
	local name
	for _, n in f.nodes do
		if n.id == "3" then
			name = n.name
		end
	end
	expect(name).to_be("Worker two")
end)

test("orch flow page: removing a block takes its connections with it", function()
	local app = open()
	app:click("#orch-flow-remove-block")
	settle(app)
	local f = saved(app)
	for _, n in f.nodes do
		expect(n.id ~= "3").to_be(true)
	end
	for _, e in f.edges do
		expect(e.from ~= "3" and e.to ~= "3").to_be(true)
	end
	-- (What's left can't run: the toolbar says so.)
	expect(app:has("#orch-flow-issues")).to_be(true)
end)

test("orch flow page: nothing selected, the flow's checks", function()
	local app = open()
	app:set_state("orch_flow_selected", {})
	settle(app)
	expect(app:has('#orch-flow-overview >> "Ready to run"')).to_be(true)
end)

test("orch flow page: no flow yet, its starters; one chosen, it's there", function()
	local app = open()
	app:command("flow.none")
	settle(app)
	expect(app:has("#orch-flow-empty")).to_be(true)
	app:click('#orch-flow-empty >> role:button "Plan → Build → Review"')
	settle(app)
	expect(app:has("#orch-flow-field")).to_be(true)
	expect(#saved(app).nodes).to_be(6)
end)

test("orch flow page: New orchestrator starts from the flow chosen", function()
	local app = open()
	app:set_state("orch_new_open", true)
	settle(app)
	expect(app:has("#orch-new-start")).to_be(true)
	app:set_state("orch_new_start", "default")
	app:type('#orch-new-dialog >> role:textbox "Name"', "Release train")
	settle(app)
	app:set_state("orch_flows", {})
	app:click('role:button "Create"')
	settle(app)
	-- (The fixture makes every new one `orc-orch`.)
	local f = saved(app)
	expect(f ~= nil).to_be(true)
	expect(f.name).to_be("Plan → Build → Review")
	-- Its plan in its notes, as an update after the create.
	local calls = app:state("orch_calls") :: string
	expect(string.find(calls, "createOrchestrator:", 1, true) ~= nil).to_be(true)
	expect(string.find(calls, "updateOrchestrator:id=orc-orch,notes=", 1, true) ~= nil).to_be(true)
	-- And the next opening starts blank again.
	expect(app:state("orch_new_start")).to_be("blank")
end)

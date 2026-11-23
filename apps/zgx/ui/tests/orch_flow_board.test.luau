-- A flow's run on the orchestrator page (ui/orch_flow_run_fixture.luau):
-- the Flow view's Run… (a task asked, then `startFlow`), where the
-- run is and Stop (`stopFlow`); and a flow edited, sent to the engine
-- (`updateOrchestrator` with its workflow) once the edits rest.

local function settle(app: any)
	app:frames(30)
end

local function open(): any
	local app = mount("orch_flow_run_fixture.luau")
	app:resize(1440, 900)
	settle(app)
	return app
end

local function flow_view(app: any)
	app:command("view.flow")
	settle(app)
end

local function calls(app: any): string
	return app:state("orch_calls") :: string
end

local function has(s: string, part: string): boolean
	return string.find(s, part, 1, true) ~= nil
end

test("orch flow board: the Flow view says where the run is, and Stop stops it", function()
	local app = open()
	flow_view(app)
	expect(app:has("#orch-flow-run")).to_be(false)
	expect(app:has('#orch-flow-run-state >> "Running: QA"')).to_be(true)
	app:click("#orch-flow-stop")
	settle(app)
	expect(has(calls(app), "stopFlow:orchestratorId=orc-orch")).to_be(true)
	-- (Stopped, it can run again.)
	expect(app:has("#orch-flow-run-state")).to_be(false)
	expect(app:has("#orch-flow-run")).to_be(true)
end)

test("orch flow board: waiting on the person, the run says so", function()
	local app = open()
	flow_view(app)
	app:command("run.blocked")
	settle(app)
	expect(app:has('#orch-flow-run-state >> "Waiting on you: QA"')).to_be(true)
end)

test("orch flow board: Run… asks for the task, then starts the run", function()
	local app = open()
	flow_view(app)
	app:command("run.none")
	settle(app)
	app:click("#orch-flow-run")
	settle(app)
	expect(app:has("#orch-flow-run-dialog")).to_be(true)
	app:type('#orch-flow-run-dialog >> role:textbox "Task"', "Ship the flow chips")
	settle(app)
	app:click('role:button "Run"')
	settle(app)
	expect(has(calls(app), "startFlow:orchestratorId=orc-orch,task=Ship the flow chips")).to_be(true)
	-- (Started: where it is, from its first step.)
	expect(app:has('#orch-flow-run-state >> "Running: Planner"')).to_be(true)
end)

test("orch flow board: no task, Run sends nothing", function()
	local app = open()
	flow_view(app)
	app:command("run.none")
	settle(app)
	app:click("#orch-flow-run")
	settle(app)
	app:click('role:button "Run"')
	settle(app)
	expect(has(calls(app), "startFlow:")).to_be(false)
end)

test("orch flow board: a flow with something to fix offers no Run…", function()
	local app = open()
	flow_view(app)
	app:command("run.none")
	settle(app)
	app:set_state("orch_flow_selected", { node = "3" })
	settle(app)
	app:click("#orch-flow-remove-block")
	settle(app)
	expect(app:has("#orch-flow-issues")).to_be(true)
	expect(app:has("#orch-flow-run")).to_be(false)
end)

test("orch flow board: an edit goes to the engine once edits rest", function()
	local app = open()
	flow_view(app)
	app:set_state("orch_flow_selected", { node = "3" })
	settle(app)
	app:type('#orch-flow-block >> role:textbox "Name"', " two")
	app:key("Enter")
	settle(app)
	-- (Not at once: a drag is many edits.)
	expect(has(calls(app), "workflow=")).to_be(false)
	app:wait("#never-shown", { timeout_ms = 900 })
	settle(app)
	expect(has(calls(app), "updateOrchestrator:id=orc-orch,workflow={…}")).to_be(true)
	local sent = app:state("orch_flow_sent") :: string
	expect(has(sent, "version=2;next_id=7;")).to_be(true)
	expect(has(sent, "3=Worker two")).to_be(true)
	-- What the engine now holds is what shows.
	expect(app:has('#orch-flow-block >> role:textbox "Name"')).to_be(true)
	expect(app:has('#orch-flow-overview')).to_be(false)
end)

test("orch flow board: the engine's flow shows over this device's", function()
	local app = open()
	flow_view(app)
	-- (Nothing kept here for it: the engine's alone.)
	expect(app:has("#orch-flow-field")).to_be(true)
	expect(app:has("#orch-flow-empty")).to_be(false)
end)

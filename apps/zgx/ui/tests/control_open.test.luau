-- Opening a crew thread from the Control room: crew threads are listed under
-- their orchestrator (`sessions.now.children`), and the route finds them
-- there (route.find_chat), so a pill, a comms row or a chip's Open opens
-- the thread as a chat of its own. In the window (ui/control_fixture.luau).

local function open(): any
	local app = mount("control_fixture.luau")
	app:resize(1440, 900)
	app:frames(40)
	return app
end

local function at(app: any): string
	local h = app:state("shell_route_history")
	return h.entries[h.at]
end

test("a crew pill on the Map opens its thread", function()
	local app = open()
	app:click('#control-widget-map >> .orch-pill "Engine: orchestration state, Testing"')
	app:frames(30)
	expect(at(app)).to_be("/session/th-engine-state")
	expect(app:has("#control-room")).to_be(false)
	expect(app:text("#page-title")).to_be("Engine: orchestration state")
end)

test("a comms row opens the crew thread it came from", function()
	local app = open()
	app:resize(1440, 2400)
	app:frames(10)
	app:click('#control-widget-comms >> role:button "Coalesced wake works in tests"')
	app:frames(30)
	expect(at(app)).to_be("/session/th-engine-flow")
	expect(app:text("#page-title")).to_be("Engine: reports + MCP tools")
end)

test("a receipt chip's Open goes to its crew thread", function()
	local app = open()
	-- (Its chip past the strip's end: the pointer on its part brings it in.)
	app:hover('#control-capture >> .orch-part >> "registry "')
	app:frames(10)
	app:click('#control-capture >> role:button "Engine: orchestration state, Question, Delivered"')
	app:frames(10)
	app:click('"Open Engine: orchestration state"')
	app:frames(30)
	expect(at(app)).to_be("/session/th-engine-state")
end)

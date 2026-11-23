-- A brand-new orchestrator, nothing in it yet (no crew, no tasks, no
-- asks), on the Control room (ui/orch_map_fixture.luau): named on the Map
-- as a hub of its own, its hover card opening its page; on the Board, in
-- its Orchestrators filter, and filtered to it, each column's empty state.

local function add_empty(app: any)
	local t = os.time()
	local now = table.clone(app:state("orchestration") :: any)
	local list = table.clone(now.orchestrators)
	table.insert(list, {
		id = "orc-token", kind = "standard", name = "Project Tokenheimer", goal = "", rules = "", crew_rules = "", notes = "",
		hue = 40, crew = { devices = {} }, paused = false, archived = false, created = t - 60, updated = t - 60,
	})
	now.orchestrators = list
	app:set_state("orchestration", now)
	-- (Its own chat, as the engine makes one with it: no messages yet.)
	local s = table.clone(app:state("sessions") :: any)
	local chats = table.clone(s.chats)
	table.insert(chats, { id = "orc-token", title = "Project Tokenheimer", device = "dev-desk", updated = t - 60, created = t - 60, agent = "claude" })
	s.chats = chats
	app:set_state("sessions", s)
end

local function open(): any
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 1600)
	app:command("map_only")
	app:set_state("control_map_force_pivot", "orchestrator")
	app:command("map_force")
	app:frames(30)
	add_empty(app)
	app:frames(90)
	app:wait(400)
	app:frames(20)
	return app
end

test("empty orchestrator: on the Map, named, its card opening its page", function()
	local app = open()
	expect(app:has('#control-map >> .orch-force-orchestrator "Project Tokenheimer"')).to_be(true)
	app:set_state("control_map_force_hover", "orc-token")
	app:frames(3)
	app:hover(".orch-force-anchor-box")
	expect(app:wait(".orch-force-card", { timeout_ms = 1500 })).to_be(true)
	app:wait(700)
	app:frames(10)
	expect(app:has('.orch-force-card >> "Project Tokenheimer"')).to_be(true)
	app:click("#orch-force-card-open-page")
	app:frames(5)
	expect(app:state("shell_route_path")).to_be("/orchestrator/orc-token")
	expect(app:error()).to_be_nil()
end)

test("empty orchestrator: on the Board's filter; filtered to it, each column says it's empty", function()
	local app = open()
	app:set_state("control_tab", "board")
	app:frames(60)
	app:click("#orch-board-filter")
	app:frames(20)
	local ITEM = 'role:menuitemcheckbox "Project Tokenheimer"'
	expect(app:has(ITEM)).to_be(true)
	app:click(ITEM)
	app:frames(30)
	expect(app:has('#orch-board-filter >> "Project Tokenheimer"')).to_be(true)
	expect(app:has('#orch-col-needs_you >> "Nothing needs you"')).to_be(true)
	expect(app:has('#orch-col-running >> "Nothing is running"')).to_be(true)
	expect(app:has('#orch-col-done >> "Nothing finished today"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

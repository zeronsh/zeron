-- Opening a crew thread from the Control room: crew threads are listed under
-- their orchestrator (`sessions.now.children`), and the route finds them
-- there (route.find_chat), so a pill on the Map or a receipt chip's Open
-- opens the thread as a chat of its own. In the window
-- (ui/control_fixture.luau).

local function open(): any
	local app = mount("control_fixture.luau")
	app:resize(1440, 900)
	app:frames(40)
	return app
end

-- Whether a hover card shows (zgx's tooltip panel, drawn: not hidden).
local function card_shows(app: any): boolean
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(' dialog "Tooltip" ', 1, true) and not line:find("hidden", 1, true) then
			return true
		end
	end
	return false
end

-- The part's card comes once the pointer has rested on its word 300 ms of
-- the wall clock, which a slow runner's frames outlast, and it hangs over
-- the chips: rest past that, then let it go (Escape), so a chip is in
-- reach on any runner.
local function rest_and_let_go(app: any)
	app:wait("#no-such-node", { timeout_ms = 400 })
	expect(card_shows(app)).to_be(true)
	app:key("Escape")
	app:frames(5)
	expect(card_shows(app)).to_be(false)
end

local function at(app: any): string
	local h = app:state("shell_route_history")
	return h.entries[h.at]
end

test("a crew pill on the Map opens its thread", function()
	local app = open()
	-- (The Map tab, in the Topology: still drawn, set once the tab is built.)
	app:set_state("control_tab", "map")
	app:frames(30)
	app:set_state("control_map", { variant = "topology", window = 15 })
	app:frames(30)
	app:click('#control-map >> .orch-pill "Engine: orchestration state, Testing"')
	app:frames(30)
	expect(at(app)).to_be("/session/th-engine-state")
	expect(app:has("#control-room")).to_be(false)
	expect(app:text("#page-title")).to_be("Engine: orchestration state")
end)

test("a receipt chip's Open goes to its crew thread", function()
	local app = open()
	-- (In Capture's history, opened from its bar.)
	app:click("#control-history-open")
	app:frames(30)
	-- (Its chip past the strip's end: the pointer on its part brings it in.)
	app:hover('#control-capture >> .orch-part >> "registry "')
	app:frames(10)
	rest_and_let_go(app)
	app:click('#control-capture >> role:button "Engine: orchestration state, Question, Delivered"')
	app:frames(10)
	app:click('"Open Engine: orchestration state"')
	app:frames(30)
	expect(at(app)).to_be("/session/th-engine-state")
end)

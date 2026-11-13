-- The sidebar's Orchestrators section, always there to make one, and what
-- its view menu ("…") offers for orchestration: a new orchestrator, the
-- section shown or put away, archived ones listed; the Control room, its
-- widgets, the Map's view, the Brain dump's model
-- (ui/orchestrator_fixture.luau: the whole window on the sample).

local SIDE = "#shell-sidebar"
local MORE = `{SIDE} >> role:button "Sidebar view options"`

local function settle(app: any)
	app:frames(30)
end

local function open(): any
	local app = mount("orchestrator_fixture.luau")
	app:resize(1440, 900)
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

-- A menu's row by its name, whichever kind it is (an item, a toggle).
local function item(app: any, label: string): string
	for _, role in { "menuitem", "checkbox", "menuitemcheckbox", "button" } do
		local sel = `role:{role} "{label}"`
		if app:has(sel) then
			return sel
		end
	end
	return `role:menuitem "{label}"`
end

-- The view menu, open, and one of its rows chosen.
local function choose(app: any, label: string)
	app:click(MORE)
	settle(app)
	-- (The menu scrolls once taller than the window: the row brought into
	-- view first.)
	app:scroll("#sidebar-view-menu-scroll", 2000)
	settle(app)
	app:click(item(app, label))
	settle(app)
end

test("sidebar orch view: with none, the section stands, its + and its row make one", function()
	local app = open()
	app:command("orch.none")
	settle(app)
	expect(shown(app, `{SIDE} >> .orchestrators-header`)).to_be(true)
	expect(app:has(`{SIDE} >> .orchestrator-row`)).to_be(false)
	expect(shown(app, `{SIDE} >> .orchestrators-first`)).to_be(true)
	app:click(`{SIDE} >> .orchestrators-first`)
	settle(app)
	expect(app:state("orch_new_open")).to_be(true)
	app:set_state("orch_new_open", false)
	settle(app)
	app:click(`{SIDE} >> .orchestrators-new >> role:button`)
	settle(app)
	expect(app:state("orch_new_open")).to_be(true)
end)

test("sidebar orch view: with some, no row to make the first", function()
	local app = open()
	expect(app:has(`{SIDE} >> .orchestrator-row`)).to_be(true)
	expect(shown(app, `{SIDE} >> .orchestrators-first`)).to_be(false)
end)

test("sidebar orch view: New orchestrator… from the view menu", function()
	local app = open()
	choose(app, "New orchestrator…")
	expect(app:state("orch_new_open")).to_be(true)
end)

test("sidebar orch view: the section put away and back, kept", function()
	local app = open()
	choose(app, "Show section")
	expect(app:state("shell_sidebar_show_orchestrators")).to_be(false)
	expect(shown(app, `{SIDE} >> .orchestrators-header`)).to_be(false)
	-- The Control room stays.
	expect(app:has(`{SIDE} >> .control-room-row`)).to_be(true)
	app:click(item(app, "Show section"))
	settle(app)
	expect(app:state("shell_sidebar_show_orchestrators")).to_be(true)
	expect(shown(app, `{SIDE} >> .orchestrators-header`)).to_be(true)
end)

test("sidebar orch view: archived orchestrators listed when asked, quieter, to unarchive", function()
	local app = open()
	app:command("orch.archive_ci")
	settle(app)
	expect(app:has(`{SIDE} >> role:button "CI speed"`)).to_be(false)
	choose(app, "Show archived")
	expect(app:state("shell_sidebar_show_archived_orchestrators")).to_be(true)
	expect(app:has(`{SIDE} >> role:button "CI speed"`)).to_be(true)
	expect(app:has(`{SIDE} >> .orchestrator-archived`)).to_be(true)
	app:key("Escape")
	settle(app)
	app:right_click(`{SIDE} >> role:button "CI speed"`)
	settle(app)
	app:click(item(app, "Unarchive"))
	settle(app)
	expect(app:state("orch_calls")).to_contain("updateOrchestrator:archived=false,id=orc-ci")
end)

test("sidebar orch view: Open Control room", function()
	local app = open()
	choose(app, "Open Control room")
	expect(app:state("orch_route")).to_be("control:")
end)

test("sidebar orch view: Widgets… shows and hides the Control room's widgets", function()
	local app = open()
	choose(app, "Widgets…")
	expect(app:has(item(app, "Comms"))).to_be(true)
	app:click(item(app, "Comms"))
	settle(app)
	local w = app:state("control_widgets") :: any
	expect(w.hidden.comms).to_be(true)
end)

test("sidebar orch view: Default map view changes the Map", function()
	local app = open()
	choose(app, "Default map view")
	app:click(item(app, "Timeline"))
	settle(app)
	expect((app:state("control_map") :: any).variant).to_be("timeline")
	choose(app, "Open Control room")
	settle(app)
	expect(app:has(".orch-map-timeline")).to_be(true)
	expect(app:has(".orch-map-topology")).to_be(false)
	choose(app, "Default map view")
	app:click(item(app, "Machines"))
	settle(app)
	expect(app:has(".orch-map-machines")).to_be(true)
end)

test("sidebar orch view: Brain dump model… opens Capture's model picker", function()
	local app = open()
	choose(app, "Open Control room")
	choose(app, "Brain dump model…")
	settle(app)
	expect(app:state("orch_route")).to_be("control:")
	expect(app:state("chat_lane_model_wanted")).to_be(nil)
	expect(app:has('role:textbox "Search"')).to_be(true)
	expect(app:has('role:option "Haiku 5.5"')).to_be(true)
end)

test("sidebar orch view: Brain dump model… from another page opens it as Capture comes", function()
	local app = open()
	expect(app:state("orch_route")).to_be("orchestrator:orc-orch")
	choose(app, "Brain dump model…")
	settle(app)
	settle(app)
	expect(app:state("orch_route")).to_be("control:")
	expect(app:has('#control-capture-composer')).to_be(true)
	expect(app:has('role:textbox "Search"')).to_be(true)
end)

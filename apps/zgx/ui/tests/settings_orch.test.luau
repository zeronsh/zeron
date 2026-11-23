-- Settings → Orchestration (screens/settings/orchestration) and the
-- sidebar view menu's quick toggle, on ui/settings_orch_fixture.luau: the
-- auto-archive switch (on by default) and its window (4 hours) render,
-- each change goes to the engine as `updateSettings`, the menu's row
-- follows the same setting, and what was set is there on the next launch
-- (the engine's row, kept).

local PAGE = "#settings-page"
local SWITCH = `{PAGE} >> role:switch "Auto-archive orchestrator chats"`
local SIDE = "#shell-sidebar"
local QUICK = 'role:checkbox "Auto-archive idle chats"'

local function open(): any
	local app = mount("settings_orch_fixture.luau")
	app:resize(1300, 900)
	app:frames(30)
	return app
end

local function switched(app: any): boolean
	return not string.find(app:tree(SWITCH) :: string, "unchecked", 1, true)
end

local function menu(app: any)
	app:click(`{SIDE} >> role:button "Sidebar view options"`)
	app:frames(20)
end

test("settings orch: the switch and the window render, on and 4 hours by default", function()
	local app = open()
	local page = app:tree(PAGE) :: string
	expect(page).to_contain("Orchestration")
	expect(page).to_contain("Auto-archive orchestrator chats")
	expect(page).to_contain("Archive after")
	expect(page).to_contain("4 hours")
	expect(switched(app)).to_be(true)
	expect(app:state("orch_ops")).to_be("")
end)

test("settings orch: the switch turns archiving off and on through the engine", function()
	local app = open()
	app:click(SWITCH)
	app:frames(20)
	expect(switched(app)).to_be(false)
	expect(app:state("orch_ops")).to_be("updateSettings:crewAutoArchive=false")
	app:click(SWITCH)
	app:frames(20)
	expect(switched(app)).to_be(true)
	expect(app:state("orch_ops")).to_be("updateSettings:crewAutoArchive=false\nupdateSettings:crewAutoArchive=true")
end)

test("settings orch: a new window goes to the engine and shows", function()
	local app = open()
	app:command("idle_24")
	app:frames(20)
	expect(app:state("orch_ops")).to_be("updateSettings:crewIdleHours=24")
	expect(app:tree(PAGE)).to_contain("1 day")
end)

test("settings orch: the sidebar menu's quick toggle is the same setting", function()
	local app = open()
	menu(app)
	expect(app:has(QUICK)).to_be(true)
	expect(string.find(app:tree(QUICK) :: string, "unchecked", 1, true)).to_be(nil)
	app:click(QUICK)
	app:frames(20)
	expect(app:state("orch_ops")).to_be("updateSettings:crewAutoArchive=false")
	expect(string.find(app:tree(QUICK) :: string, "unchecked", 1, true) ~= nil).to_be(true)
	app:key("Escape")
	app:frames(20)
	expect(switched(app)).to_be(false)
end)

test("settings orch: what was set is there on the next launch", function()
	local first = open()
	first:click(SWITCH)
	first:frames(10)
	first:command("idle_24")
	first:frames(20)
	-- (A second mount is the next launch: the engine's row kept.)
	local app = open()
	expect(switched(app)).to_be(false)
	expect(app:tree(PAGE)).to_contain("1 day")
	expect(app:state("orch_ops")).to_be("")
end)

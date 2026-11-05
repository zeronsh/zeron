-- A repository's clones and worktrees, on any device, are one project in the sidebar,
-- named by the oldest; a new chat in it starts in the clone on this device; removing it
-- asks which devices to remove it from. On the shell in ui/shell_fixture.luau.

local SIDE = "#shell-sidebar"
local ORGANIZE = "screens/shell/sidebar.luau::shell_sidebar_organize"
local CHOSEN = "screens/shell/sidebar.luau::shell_project_remove_chosen"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	app:command("atlas_cloned_on_box")
	app:set_state(ORGANIZE, { "project" })
	app:frames(40)
	return app
end

test("a repository's clones are one project, with the sessions of them all", function()
	local app = open()
	local side = app:tree(SIDE)
	expect(side).to_contain("Atlas")
	expect(side).to_contain("On the clone")
	expect(string.find(side, "atlas-clone", 1, true)).to_be(nil)
	expect(side).to_contain("Fix the build")
	expect(app:error()).to_be_nil()
end)

test("a new chat in it starts in the clone on this device", function()
	local app = open()
	app:hover(`{SIDE} >> role:button "Atlas"`)
	app:frames(20)
	app:click(`{SIDE} >> role:button "New chat in Atlas"`)
	app:frames(40)
	expect(app:state("chat_project")).to_be("atlas")
	expect(app:error()).to_be_nil()
end)

test("removing it asks which devices, this one ticked to begin with", function()
	local app = open()
	app:right_click(`{SIDE} >> role:button "Atlas"`)
	app:frames(20)
	expect(app:wait('role:menuitem "Remove…"', { timeout_ms = 5000 })).to_be(true)
	app:click('role:menuitem "Remove…"')
	app:frames(60)
	expect(app:has('role:checkbox ~"MacBook Pro"')).to_be(true)
	expect(app:has('role:checkbox ~"devbox"') or app:has('role:checkbox ~"Build box"')).to_be(true)
	local chosen = app:state(CHOSEN) :: any
	expect(chosen.atlas).to_be(true)
	expect(chosen["atlas-clone"]).to_be_nil()
	expect(app:error()).to_be_nil()
end)

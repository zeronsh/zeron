-- Deleting a session from the sidebar the host refuses: the window has
-- already left it (it was open), so a toast says it wasn't deleted, and the
-- row stays. On ui/shell_fixture.luau (`session_delete_fails`: the host
-- refuses "Tidy the tests").

local DELETE_ONE = `role:dialog "Delete session?"`
local ROW = '#shell-sidebar >> role:button "Tidy the tests"'

local function delete(app: any)
	app:right_click(ROW)
	app:frames(20)
	app:click('role:menuitem "Delete…"')
	app:frames(30)
	app:click(`{DELETE_ONE} >> role:button "Delete"`)
	app:frames(40)
end

test("a refused delete says so, and the session stays", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("session_delete_fails")
	app:click(ROW)
	app:frames(30)
	delete(app)
	expect(app:has('"Couldn’t delete “Tidy the tests”"')).to_be(true)
	expect(app:has(ROW)).to_be(true)
end)

test("a delete the host makes says nothing", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	delete(app)
	expect(app:has(ROW)).to_be(false)
	expect(app:has('"Couldn’t delete “Tidy the tests”"')).to_be(false)
end)

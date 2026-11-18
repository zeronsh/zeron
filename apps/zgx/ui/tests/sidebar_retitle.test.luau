-- "Regenerate title" on a session's menu (RetitleChat): shown while an
-- enabled agent can title, the call the host gets, and refused, its words
-- in a toast. Without one (settings_stub.titles_off) hidden — the answer
-- is asked again as the menu opens, so the item goes on its next open.

local ROW = '#shell-sidebar >> role:button "Tidy the tests"'
local ITEM = 'role:menuitem "Regenerate title"'

test("Regenerate title asks the host to name the session", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:right_click(ROW)
	app:frames(20)
	app:click(ITEM)
	app:frames(20)
	expect(app:state("calls") or "").to_contain("retitle:a3")
end)

test("a refused retitle says the engine's words", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("session_retitle_fails")
	app:right_click(ROW)
	app:frames(20)
	app:click(ITEM)
	app:frames(30)
	expect(app:has('"Couldn’t retitle “Tidy the tests”"')).to_be(true)
	expect(app:has('"no title could be generated"')).to_be(true)
end)

test("Regenerate title hides when no agent can title", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.titles_off")
	app:right_click(ROW)
	app:frames(20)
	app:key("Escape")
	app:frames(20)
	app:right_click(ROW)
	app:frames(20)
	expect(app:has(ITEM)).to_be(false)
	expect(app:has('role:menuitem "Rename"')).to_be(true)
end)

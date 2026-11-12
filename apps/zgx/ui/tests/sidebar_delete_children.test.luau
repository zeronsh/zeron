-- Deleting a session deletes its subagents and side chats, and theirs in
-- turn, so "Delete session?" says what goes with it, as the host counts it
-- (src/sessions.rs `deleted_with`, archived ones too, never a computer's
-- Cloud delegate). With nothing going with it, the dialog reads as it
-- always did. On ui/shell_fixture.luau (`tidy_side_chats`, `tidy_subagent`).

local DIALOG = `role:dialog "Delete session?"`
local ROW = '#shell-sidebar >> role:button "Tidy the tests"'
local WILL = "“Tidy the tests” will be permanently deleted. This can’t be undone."

local function ask(app: any)
	app:right_click(ROW)
	app:frames(20)
	app:click('role:menuitem "Delete…"')
	app:frames(30)
end

local function open(commands: { string }): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	for _, command in commands do
		app:command(command)
	end
	app:frames(10)
	ask(app)
	return app
end

test("a session with nothing under it reads as before", function()
	local app = open({})
	expect(app:has(`{DIALOG} >> "{WILL}"`)).to_be(true)
end)

test("its side chats are said, as many as go", function()
	local app = open({ "tidy_side_chats" })
	expect(app:has(`{DIALOG} >> "{WILL} This also deletes its 3 side chats."`)).to_be(true)
end)

test("subagents and side chats are named both", function()
	local app = open({ "tidy_side_chats", "tidy_subagent" })
	expect(app:has(`{DIALOG} >> "{WILL} This also deletes its 1 subagent and 3 side chats."`)).to_be(true)
end)

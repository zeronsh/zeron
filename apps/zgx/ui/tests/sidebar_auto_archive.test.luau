-- Auto-archive follows the days set in every sidebar view, not only in
-- Activity: in the project-grouped view an idle chat goes to Archived too,
-- Never leaves them all, and a pinned chat stays. The view menu sets the
-- days too, in either view. On the shell in
-- ui/shell_fixture.luau, with the Activity view off.

local SIDE = "#shell-sidebar"
local ON = "screens/shell/activity.luau::shell_sidebar_activity"
local AFTER = "screens/shell/activity.luau::activity_archive_after"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:set_state(ON, false)
	app:frames(40)
	return app
end

-- Whether the sidebar shows a row starting with `label`.
local function shows(app: any, label: string): boolean
	return app:tree(SIDE):find(` button "{label}`, 1, true) ~= nil
end

local function settle(app: any)
	app:frames(40)
end

-- Real time passing (the view's timers run on it).
local function wait_ms(app: any, ms: number)
	app:wait('"never shown, only waited past"', { timeout_ms = ms })
	settle(app)
end

test("Auto-archive: in the project view, idle chats are archived after the days set", function()
	local app = open()
	expect(shows(app, "Atlas")).to_be(true)
	-- Answer the review last moved a day ago (to the second: past it a
	-- moment on); the rest within hours.
	wait_ms(app, 1100)
	app:set_state(AFTER, 1)
	wait_ms(app, 300)
	expect(shows(app, "Answer the review")).to_be(false)
	expect(shows(app, "Write the docs")).to_be(true)
	expect(shows(app, "Ship the release")).to_be(true)
end)

test("Auto-archive: Never archives nothing in the project view", function()
	local app = open()
	wait_ms(app, 1100)
	app:set_state(AFTER, 0)
	wait_ms(app, 300)
	expect(shows(app, "Answer the review")).to_be(true)
end)

test("Auto-archive: a pinned chat stays", function()
	local app = open()
	-- (Pinned as a drop on Pinned puts it: Answer the review is b2.)
	app:set_state("shell_places", { pinned = { "b2" }, sections = {} })
	settle(app)
	wait_ms(app, 1100)
	app:set_state(AFTER, 1)
	wait_ms(app, 300)
	expect(shows(app, "Answer the review")).to_be(true)
end)

test("Auto-archive: the view menu's Archive idle chats picks the days", function()
	local app = open()
	app:set_state(AFTER, 0)
	settle(app)
	app:click(`{SIDE} >> role:button "Sidebar view options"`)
	settle(app)
	-- Its row says the days set, and opens their choices beside the menu.
	expect(app:has('role:menuitem "Archive idle chats"')).to_be(true)
	app:hover('role:menuitem "Archive idle chats"')
	settle(app)
	expect(app:has('role:option "Never"')).to_be(true)
	app:click('role:option "After 7 days"')
	settle(app)
	expect(app:state(AFTER)).to_be(7)
	-- Settings → Activity says the same.
	app:command("shell.settings")
	app:frames(20)
	app:click('#settings-nav >> role:tab "Activity"')
	settle(app)
	expect(app:tree("#settings-page"):find("After 7 days", 1, true) ~= nil).to_be(true)
end)

test("Auto-archive: the view menu's row is in reach in Activity too", function()
	local app = open()
	app:set_state(ON, true)
	settle(app)
	app:click(`{SIDE} >> role:button "Sidebar view options"`)
	settle(app)
	local line = app:tree():match('[^\n]*menuitem "Archive idle chats"[^\n]*') or ""
	expect(line ~= "").to_be(true)
	expect(line:find("disabled", 1, true)).to_be(nil)
end)

test("Auto-archive: Settings' Archive idle chats is in reach with Activity off", function()
	local app = open()
	app:command("shell.settings")
	app:frames(20)
	app:click('#settings-nav >> role:tab "Activity"')
	settle(app)
	-- With the view off, the row is not dimmed or disabled.
	local tree = app:tree("#settings-page")
	local line = tree:match('[^\n]*button "Archive idle chats"[^\n]*') or ""
	expect(line ~= "").to_be(true)
	expect(line:find("disabled", 1, true)).to_be(nil)
end)

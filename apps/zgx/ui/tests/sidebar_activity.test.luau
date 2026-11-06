-- The Activity sidebar: one list of what needs you, the one back last at the
-- top, no groups or sections; Working, Asleep and Archived folded at the
-- list's foot, each opened by its title and kept so. A chat under way
-- (running, or waiting on background work that will wake it) is in
-- Working, and back at the top once it's done; Stop waiting releases a
-- wait. One put to sleep waits in Asleep, its corner counting down, and
-- shows the alarm once it wakes. (When it wakes, and that it syncs, are the
-- engine's: its tests.) The view menu turns it on and off, opens its
-- settings, and holds what it stands aside from out of reach. Off, the
-- sidebar is as it was. On the shell in ui/shell_fixture.luau.

local SIDE = "#shell-sidebar"
local ON = "screens/shell/activity.luau::shell_sidebar_activity"
local FOLDED = "screens/shell/sidebar.luau::shell_sidebar_folded"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:set_state(ON, true)
	app:frames(40)
	return app
end

-- Whether the sidebar shows a row starting with `label`.
local function shows(app: any, label: string): boolean
	return app:tree(SIDE):find(` button "{label}`, 1, true) ~= nil
end

-- Where a row stands (its top), by the start of its label.
local function top(app: any, label: string): number
	for line in app:tree(SIDE):gmatch("[^\n]+") do
		if line:find(` button "{label}`, 1, true) then
			local y = line:match(" %-?%d+,(%-?%d+) ")
			return tonumber(y) :: number
		end
	end
	error(`no "{label}" in the sidebar`)
end

local function settle(app: any)
	app:frames(40)
end

-- Real time passing (the view's timers run on it).
local function wait_ms(app: any, ms: number)
	app:wait('"never shown, only waited past"', { timeout_ms = ms })
	settle(app)
end

test("Activity: Activity lists every chat newest first, no groups, its shelves folded at the foot", function()
	local app = open()
	expect(shows(app, "Activity")).to_be(true)
	-- (No project headers: one list.)
	expect(shows(app, "Atlas")).to_be(false)
	expect(shows(app, "Borealis")).to_be(false)
	-- The one that moved last first.
	expect(top(app, "Write the docs") < top(app, "Tidy the tests")).to_be(true)
	expect(top(app, "Tidy the tests") < top(app, "Ship the release")).to_be(true)
	expect(top(app, "Ship the release") < top(app, "Answer the review")).to_be(true)
	-- Working (Fix the build) and Archived (Old experiment): folded, at the
	-- foot, under the list with room between.
	expect(shows(app, "Working 1")).to_be(true)
	expect(shows(app, "Archived 1")).to_be(true)
	expect(shows(app, "Fix the build")).to_be(false)
	expect(shows(app, "Old experiment")).to_be(false)
	expect(top(app, "Working 1") - top(app, "Answer the review") > 200).to_be(true)
	expect(top(app, "Working 1") < top(app, "Archived 1")).to_be(true)
end)

test("Activity: a shelf opens from its title and stays open", function()
	local app = open()
	app:click(`{SIDE} >> role:button "Working 1"`)
	settle(app)
	expect(shows(app, "Fix the build")).to_be(true)
	expect(((app:state(FOLDED) :: any) or {})["::shelf:working"]).to_be(true)
	app:click(`{SIDE} >> role:button "Working 1"`)
	settle(app)
	expect(shows(app, "Fix the build")).to_be(false)
	expect(((app:state(FOLDED) :: any) or {})["::shelf:working"]).to_be(nil)
end)

test("Activity: a chat that stops working is back at the top", function()
	local app = open()
	expect(shows(app, "Fix the build")).to_be(false)
	app:command("finish")
	settle(app)
	expect(shows(app, "Working")).to_be(false)
	expect(shows(app, "Fix the build")).to_be(true)
	expect(top(app, "Fix the build") < top(app, "Write the docs")).to_be(true)
end)

test("Activity: a chat waiting on its tests is under way, and Stop waiting brings it back", function()
	local app = open()
	app:command("wait_on_tests")
	settle(app)
	expect(shows(app, "Working 2")).to_be(true)
	expect(shows(app, "Write the docs")).to_be(false)
	app:click(`{SIDE} >> role:button "Working 2"`)
	settle(app)
	app:right_click(`{SIDE} >> role:button "Write the docs"`)
	settle(app)
	app:click('"Stop waiting"')
	settle(app)
	-- Back in the list, over the shelf; the build still under way in it.
	expect(top(app, "Write the docs") < top(app, "Working")).to_be(true)
	expect(top(app, "Working") < top(app, "Fix the build")).to_be(true)
end)

test("Activity: a chat that starts working folds into Working", function()
	local app = open()
	app:command("work")
	settle(app)
	expect(shows(app, "Working 2")).to_be(true)
	expect(shows(app, "Write the docs")).to_be(false)
end)

test("Activity: a chat asleep waits in Asleep, saying when it wakes, and wears the alarm once it wakes", function()
	local app = open()
	app:command("sleep_a3")
	settle(app)
	expect(shows(app, "Asleep 1")).to_be(true)
	expect(shows(app, "Tidy the tests")).to_be(false)
	app:click(`{SIDE} >> role:button "Asleep 1"`)
	settle(app)
	expect(shows(app, "Tidy the tests")).to_be(true)
	expect(app:tree(SIDE):find('text "3h"', 1, true) ~= nil).to_be(true)
	-- It woke: back in the list, at the top, wearing the alarm.
	app:command("woke")
	settle(app)
	expect(shows(app, "Asleep")).to_be(false)
	expect(top(app, "Tidy the tests") < top(app, "Write the docs")).to_be(true)
	expect(app:tree(SIDE):find('"Woke from sleep"', 1, true) ~= nil).to_be(true)
end)

test("Activity: Sleep's times are in the chat's menu, and Wake once it's asleep", function()
	local app = open()
	app:right_click(`{SIDE} >> role:button "Write the docs"`)
	settle(app)
	app:hover('"Sleep"')
	settle(app)
	expect(app:has('~"In 1 hour"')).to_be(true)
	expect(app:has('"Custom…"')).to_be(true)
	app:click('~"In 1 hour"')
	settle(app)
	expect(shows(app, "Asleep 1")).to_be(true)
	app:click(`{SIDE} >> role:button "Asleep 1"`)
	settle(app)
	app:right_click(`{SIDE} >> role:button "Write the docs"`)
	settle(app)
	app:click('"Wake"')
	settle(app)
	expect(shows(app, "Asleep")).to_be(false)
end)

test("Activity: the view menu opens its settings, and holds what it stands aside from out of reach", function()
	local app = open()
	app:click(`{SIDE} >> role:button "Sidebar view options"`)
	settle(app)
	-- Its own row, checked; grouping, sorting, the empty projects' fold and
	-- a new section, out of reach.
	local function line(label: string): string
		for l in app:tree():gmatch("[^\n]+") do
			if l:find(`"{label}"`, 1, true) and not l:find(" button \"Activity\" 13,", 1, true) then
				return l
			end
		end
		return ""
	end
	expect(line("Activity"):find("checked", 1, true) ~= nil).to_be(true)
	expect(line("By device"):find("disabled", 1, true) ~= nil).to_be(true)
	expect(line("Last updated"):find("disabled", 1, true) ~= nil).to_be(true)
	expect(line("Empty projects"):find("disabled", 1, true) ~= nil).to_be(true)
	expect(line("Create section"):find("disabled", 1, true) ~= nil).to_be(true)
	-- Its settings: their page.
	app:click('role:button "Activity settings…"')
	settle(app)
	expect(app:has('"Activity sidebar"')).to_be(true)
	expect(app:has('"Archive idle chats"')).to_be(true)
end)

test("Activity: turned off from the view menu, the sidebar is grouped again", function()
	local app = open()
	app:click(`{SIDE} >> role:button "Sidebar view options"`)
	settle(app)
	app:click('role:checkbox "Activity"')
	settle(app)
	expect(app:state(ON)).to_be(false)
	expect(shows(app, "Atlas")).to_be(true)
end)

test("Activity: Activity off, the sidebar is grouped as it was", function()
	local app = open()
	app:set_state(ON, false)
	settle(app)
	expect(shows(app, "Atlas")).to_be(true)
	expect(shows(app, "Working")).to_be(false)
	expect(shows(app, "Activity")).to_be(false)
end)

test("Activity: idle chats are archived after the days set, never one waiting on you", function()
	local app = open()
	-- Answer the review last moved a day ago (to the second: past it a
	-- moment on); the rest within hours.
	wait_ms(app, 1100)
	app:set_state("screens/shell/activity.luau::activity_archive_after", 1)
	wait_ms(app, 300)
	expect(shows(app, "Answer the review")).to_be(false)
	expect(shows(app, "Archived 2")).to_be(true)
	expect(shows(app, "Write the docs")).to_be(true)
	expect(shows(app, "Ship the release")).to_be(true)
end)

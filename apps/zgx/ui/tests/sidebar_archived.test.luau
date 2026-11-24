-- An archived session on the Archived shelf reads as it did in the list:
-- its project's icon, its place on regular's second line, its card. A row
-- is its session's wherever it's listed, so archived and brought back (by
-- its own button, again and again) it follows: the button it offers, its
-- icon. On ui/shell_fixture.luau ("Old experiment", Atlas's, archived).

local SIDE = "#shell-sidebar"
local ACTIVITY = "screens/shell/activity.luau::shell_sidebar_activity"
local SHOWN = "screens/shell/sidebar.luau::shell_sidebar_show_archived"
local FOLDED = "screens/shell/sidebar.luau::shell_sidebar_folded"
local COMPACT = "screens/shell/sidebar.luau::shell_sidebar_compact"
local OLD = `{SIDE} >> role:button "Old experiment"`

local function open(activity: boolean): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:set_state(ACTIVITY, activity)
	app:set_state(SHOWN, true)
	app:frames(40)
	if not app:has(OLD) then
		app:click(`{SIDE} >> "Archived"`)
		app:frames(60)
	end
	return app
end
-- The button in its corner, under the pointer.
local function press(app: any, name: string)
	app:hover(OLD)
	app:frames(20)
	app:click(`{OLD} >> role:button "{name}"`)
	app:frames(60)
end
local function offers(app: any, name: string): boolean
	app:hover(OLD)
	app:frames(20)
	return app:has(`{OLD} >> role:button "{name}"`)
end

for _, activity in { true, false } do
	local where = if activity then "Activity" else "grouped"
	test(`{where}: brought back, it can be archived again, and brought back again`, function()
		local app = open(activity)
		for _ = 1, 2 do
			expect(offers(app, "Unarchive")).to_be(true)
			press(app, "Unarchive")
			expect(app:has(OLD)).to_be(true)
			expect(offers(app, "Unarchive")).to_be(false)
			expect(offers(app, "Archive")).to_be(true)
			press(app, "Archive")
			expect(app:has(OLD)).to_be(true)
		end
	end)
end

test("on the shelf it has its project's icon, and keeps it brought back", function()
	local app = open(true)
	expect(app:has(`{OLD} >> "A"`)).to_be(true)
	press(app, "Unarchive")
	expect(app:has(`{OLD} >> "A"`)).to_be(true)
	-- (Grouped by project, its group's icon is enough; on the shelf, its own.)
	app = open(false)
	expect(app:has(`{OLD} >> "A"`)).to_be(true)
	press(app, "Unarchive")
	expect(app:has(`{OLD} >> "A"`)).to_be(false)
end)

test("on the shelf its second line says its project", function()
	local app = open(true)
	app:set_state(COMPACT, false)
	app:frames(60)
	expect(app:has(`{OLD} >> ~"Atlas"`)).to_be(true)
end)

test("its card is its own: its title, when it moved, its project", function()
	local app = open(true)
	app:hover(OLD)
	expect(app:wait('.session-card >> "Old experiment"', { timeout_ms = 3000 })).to_be(true)
	expect(app:has('.session-card >> "2d ago"')).to_be(true)
	expect(app:has('.session-card >> "Atlas"')).to_be(true)
end)

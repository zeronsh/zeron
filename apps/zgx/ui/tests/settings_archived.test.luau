-- Settings → Archived (screens/settings/archived): the sessions put away,
-- grouped by when each last moved and searched from one field; each row the
-- sidebar's, restored or deleted from its corner or its menu; Delete all
-- asking once more, naming how many and the Cloud machines that go with
-- them, deleting only those still archived; the empty page; the header on
-- the cards' text edge, its Delete all at the title's end until the page
-- is narrow. On the shell in ui/shell_fixture.luau (`archived_days`: 37
-- sessions over the days, two on Cloud machines of their own).

local PAGE = "#settings-page"
local DELETE_ALL = `role:dialog "Delete all archived sessions?"`
local DELETE_ONE = `role:dialog "Delete session?"`

local function open(command: string?): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command(command or "archived_days")
	app:command("shell.settings")
	app:frames(10)
	app:click('#settings-nav >> "Archived"')
	app:frames(30)
	return app
end

-- A node's first line under `target`: its `@id`, and its box.
local function line(app: any, target: string): string
	return (app:tree(target):match("[^\n]+")) :: string
end
local function id_of(app: any, target: string): string
	return (line(app, target):match("^%s*@(%d+)")) :: string
end
local function box(app: any, target: string): { x: number, y: number, w: number, h: number }
	local x, y, w, h = line(app, target):match("(%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
	return { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number }
end
local function row(title: string): string
	return `{PAGE} >> role:group "{title}"`
end
local function calls(app: any): string
	return app:state("calls") :: string
end

test("sessions group by when they last moved, today first", function()
	local app = open()
	local order = {}
	for l in app:tree(PAGE):gmatch("[^\n]+") do
		local name = l:match('heading "([^"]+)"')
		if name and name ~= "Archived" then
			table.insert(order, name)
		end
	end
	expect(table.concat(order, ",")).to_be("Today,Yesterday,This week,Earlier")
	-- Each under its own: the latest today, the oldest earlier.
	expect(app:has(`{PAGE} >> "Today" >> "Port the sidebar fold"`)).to_be(true)
	expect(app:has(`{PAGE} >> "Yesterday" >> "Investigate memory spike"`)).to_be(true)
	expect(app:has(`{PAGE} >> "This week" >> "Cloud machine warmup"`)).to_be(true)
	expect(app:has(`{PAGE} >> "Earlier" >> "Audit the icon set"`)).to_be(true)
	expect(app:has(`{PAGE} >> "Today" >> "Audit the icon set"`)).to_be(false)
	-- Where each ran: its project and computer; a Cloud machine's unnamed.
	expect(app:has(`{PAGE} >> "Atlas · MacBook Pro"`)).to_be(true)
	expect(app:has(`{PAGE} >> "Borealis · devbox"`)).to_be(true)
	expect(app:tree(row("Investigate memory spike"))).to_contain('"Atlas"')
	expect(app:tree(row("Investigate memory spike"))).never.to_contain("cloud-")
	-- The count in the header.
	expect(app:has(`{PAGE} >> "37 sessions, hidden from the sidebar until restored."`)).to_be(true)
end)

test("the search narrows the rows, and says when nothing matches", function()
	local app = open()
	app:type(`{PAGE} >> role:textbox "Search archived sessions"`, "scroll")
	app:frames(30)
	local kept = id_of(app, row("Bisect the scroll bug"))
	expect(app:has(row("Bisect the scroll bug"))).to_be(true)
	expect(app:has(row("Port the sidebar fold"))).to_be(false)
	-- (Where it ran is searched too.)
	app:fill(`{PAGE} >> role:textbox "Search archived sessions"`, "devbox")
	app:frames(30)
	expect(app:has(row("Draft release notes"))).to_be(true)
	expect(app:has(row("Port the sidebar fold"))).to_be(false)
	-- Cleared: every row back, the one shown all along the same node.
	app:fill(`{PAGE} >> role:textbox "Search archived sessions"`, "")
	app:frames(30)
	expect(app:has(row("Port the sidebar fold"))).to_be(true)
	expect(id_of(app, row("Bisect the scroll bug"))).to_be(kept)
	app:fill(`{PAGE} >> role:textbox "Search archived sessions"`, "zzz")
	app:frames(30)
	expect(app:has(`#archived-none-match >> "No archived sessions match “zzz”"`)).to_be(true)
	expect(app:has(row("Bisect the scroll bug"))).to_be(false)
end)

test("a row's corner restores it; the rest stay the same nodes", function()
	local app = open()
	local other = id_of(app, row("Fix flaky login test"))
	-- At rest, its time; under the pointer, Restore and Delete in its place.
	expect(app:has(`{row("Port the sidebar fold")} >> role:button "Restore to sidebar"`)).to_be(false)
	app:hover(row("Port the sidebar fold"))
	app:frames(20)
	expect(id_of(app, row("Fix flaky login test"))).to_be(other)
	expect(app:has(`{row("Port the sidebar fold")} >> role:button "Delete"`)).to_be(true)
	app:click(`{row("Port the sidebar fold")} >> role:button "Restore to sidebar"`)
	app:frames(30)
	expect(calls(app)).to_contain("unarchive:ar1")
	expect(app:has(row("Port the sidebar fold"))).to_be(false)
	expect(id_of(app, row("Fix flaky login test"))).to_be(other)
	expect(app:has(`{PAGE} >> "36 sessions, hidden from the sidebar until restored."`)).to_be(true)
end)

test("its menu opens it, restores it, or deletes it once asked", function()
	local app = open()
	app:right_click(row("Draft release notes"))
	app:frames(20)
	expect(app:has('role:menuitem "Open"')).to_be(true)
	expect(app:has('role:menuitem "Restore to sidebar"')).to_be(true)
	app:click('role:menuitem "Delete…"')
	app:frames(30)
	expect(app:has(`{DELETE_ONE} >> "“Draft release notes” will be permanently deleted. This can’t be undone."`)).to_be(true)
	expect(calls(app)).never.to_contain("delete:ar3")
	app:click(`{DELETE_ONE} >> role:button "Delete"`)
	app:frames(30)
	expect(calls(app)).to_contain("delete:ar3")
	expect(app:has(row("Draft release notes"))).to_be(false)
	-- And from the corner: asked the same.
	app:hover(row("Fix flaky login test"))
	app:frames(20)
	app:click(`{row("Fix flaky login test")} >> role:button "Delete"`)
	app:frames(30)
	expect(app:has(DELETE_ONE)).to_be(true)
end)

test("Delete all names how many and their Cloud machines, and deletes those still archived", function()
	local app = open()
	app:click("#archived-delete-all")
	app:frames(30)
	expect(app:has(`{DELETE_ALL} >> "37 archived sessions, and 2 Cloud machines, will be permanently deleted. This can’t be undone."`)).to_be(
		true
	)
	-- One restored on another device meanwhile: it stays.
	app:command("archived_restored_elsewhere")
	app:frames(10)
	app:click(`{DELETE_ALL} >> role:button "Delete all"`)
	app:frames(40)
	local said = calls(app)
	expect(said).never.to_contain("delete:ar1,")
	local n = 0
	for _ in said:gmatch("delete:ar%d+") do
		n += 1
	end
	expect(n).to_be(36)
	expect(app:has("#archived-empty")).to_be(true)
	expect(app:has("#archived-delete-all")).to_be(false)
end)

test("Delete all's line for one session on Cloud", function()
	local app = open("archived_one_cloud")
	app:click("#archived-delete-all")
	app:frames(30)
	expect(app:has(`{DELETE_ALL} >> "1 archived session and its Cloud machine will be permanently deleted. This can’t be undone."`)).to_be(
		true
	)
end)

test("Delete all says how many it couldn't delete", function()
	local app = open()
	app:command("archived_delete_fails")
	app:click("#archived-delete-all")
	app:frames(30)
	app:click(`{DELETE_ALL} >> role:button "Delete all"`)
	app:frames(40)
	expect(app:has('"Couldn’t delete 2 archived sessions. Try again."')).to_be(true)
	expect(app:has(row("Fix flaky login test"))).to_be(true)
	expect(app:has(row("Draft release notes"))).to_be(true)
	expect(app:has(row("Port the sidebar fold"))).to_be(false)
end)

test("Escape closes Delete all, and the page stays", function()
	local app = open()
	app:click("#archived-delete-all")
	app:frames(30)
	expect(app:has(DELETE_ALL)).to_be(true)
	app:key("Escape")
	app:frames(10)
	-- (Closed: its panel then leaves on the clock, not the test's frames.)
	expect(app:state("settings_archived_delete_all_open")).to_be(false)
	expect(app:state("shell_route_path")).to_be("/settings/archived")
	expect(calls(app)).never.to_contain("delete:")
end)

test("nothing archived: a quiet empty page, no Delete all or search", function()
	local app = open("archived_none")
	expect(app:has(`#archived-empty >> "Nothing archived"`)).to_be(true)
	expect(app:has(`{PAGE} >> "Hidden from the sidebar until restored."`)).to_be(true)
	expect(app:has("#archived-delete-all")).to_be(false)
	expect(app:has("#archived-search")).to_be(false)
end)

test("past fifty, Show more shows the rest", function()
	local app = open()
	app:command("archive_many")
	app:frames(30)
	expect(app:has(row("Old 30"))).to_be(false)
	-- (At the page's foot.)
	app:set_state("shell_settings_scroll", 4000)
	app:frames(30)
	app:click("#archived-more")
	app:frames(30)
	expect(app:has(row("Old 30"))).to_be(true)
	expect(app:has("#archived-more")).to_be(false)
end)

test("the header's words and the groups' names sit on the cards' text edge", function()
	local app = open()
	-- (The card is its section's width: the section's box gives its edges.)
	local card = box(app, `{PAGE} >> role:group "Today"`)
	local title = box(app, `{PAGE} >> role:heading "Archived"`)
	local today = box(app, `{PAGE} >> role:heading "Today"`)
	expect(math.abs(title.x - (card.x + 17)) < 0.5).to_be(true)
	expect(math.abs(today.x - (card.x + 17)) < 0.5).to_be(true)
	-- A row's wash: the card's inner box edge to edge, no inset (and no
	-- corner of its own: Settings.CatalogRow's, the card's clipping it).
	local wash = box(app, `{PAGE} >> role:group "Today" >> .catalog-row[1]`)
	local first = box(app, row("Port the sidebar fold"))
	expect(first.x).to_be(card.x + 1)
	expect(first.w).to_be(card.w - 2)
	expect(wash.x).to_be(first.x)
	expect(wash.w).to_be(first.w)
	-- Its project's tile (28, its initial centred) on the text edge, as a
	-- plugin's is.
	local initial = box(app, `{row("Port the sidebar fold")} >> "A"`)
	expect(math.abs(initial.x + initial.w / 2 - (card.x + 17 + 14)) < 1).to_be(true)
	-- Delete all on the title's line, its ink (inside the ghost's 10) on the
	-- cards' end edge.
	local delete = box(app, "#archived-delete-all")
	expect(math.abs((delete.y + delete.h / 2) - (title.y + title.h / 2)) < 2).to_be(true)
	expect(math.abs((delete.x + delete.w - 10) - (card.x + card.w - 17)) < 0.5).to_be(true)
end)

test("narrow, Delete all goes under the title", function()
	local app = open()
	app:resize(560, 900)
	app:frames(30)
	local title = box(app, `{PAGE} >> role:heading "Archived"`)
	local delete = box(app, "#archived-delete-all")
	expect(delete.y > title.y + title.h).to_be(true)
	-- Its ink (inside the ghost's 10) on the words' edge.
	expect(math.abs(delete.x + 10 - title.x) < 0.5).to_be(true)
	app:resize(1200, 900)
	app:frames(30)
	delete = box(app, "#archived-delete-all")
	title = box(app, `{PAGE} >> role:heading "Archived"`)
	expect(delete.y < title.y + title.h).to_be(true)
end)

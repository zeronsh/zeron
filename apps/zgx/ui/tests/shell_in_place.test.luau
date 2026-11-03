-- The main window's shell follows the engine's sessions in place: a
-- session's state, title and time moving, one arriving, a project
-- arriving, leave every other row (and the row itself) the same node, and
-- the sidebar where it was scrolled. The route moves with the sidebar's
-- choice, back and forward, and settings; where sessions are put by hand
-- goes to the host and comes back. On the shell in ui/shell_fixture.luau.

-- A node's identity under `scope`, by its kind and label: `@id`.
local function node(app: any, scope: string, kind: string, label: string): string?
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(` {kind} "{label}"`, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

-- Where a node stands (its top), by its label.
local function top(app: any, scope: string, label: string): number?
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(`"{label}"`, 1, true) then
			local y = line:match(" %-?%d+,(%-?%d+) ")
			return tonumber(y)
		end
	end
	return nil
end

-- The same, for a row that must be there (to compare against another's).
local function top_of(app: any, scope: string, label: string): number
	local y = top(app, scope, label)
	if y == nil then
		error(`no "{label}" in {scope}`)
	end
	return y
end

-- What a row shows, its words and glyphs' names, as one line.
local function row_text(app: any, label: string): string
	return app:tree(`#shell-sidebar >> role:button "{label}"`)
end

local SIDE = "#shell-sidebar"

-- Whether session `id`'s panel is open.
local function open_panel(app: any, id: string): boolean
	return ((app:state("shell_panel_open") :: any) or {})[id] == true
end

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	return app
end

test("a session's state, title and arrival reach its row, every row staying its node", function()
	local app = open()
	local fix = node(app, SIDE, "button", "Fix the build")
	local docs = node(app, SIDE, "button", "Write the docs")
	local tidy = node(app, SIDE, "button", "Tidy the tests")
	local atlas = node(app, SIDE, "button", "Atlas")
	local borealis = node(app, SIDE, "button", "Borealis")
	for _, id in { fix, docs, tidy, atlas, borealis } do
		expect(id).never.to_be_nil()
	end
	expect(row_text(app, "Write the docs")).never.to_contain("Working")

	-- Write the docs starts working: its corner says so, in place.
	app:command("work")
	app:frames(20)
	expect(`docs @{node(app, SIDE, "button", "Write the docs")}`).to_be(`docs @{docs}`)
	expect(row_text(app, "Write the docs")).to_contain("Working")

	-- Fix the build finishes: done, in place.
	app:command("finish")
	app:frames(20)
	expect(`fix @{node(app, SIDE, "button", "Fix the build")}`).to_be(`fix @{fix}`)
	expect(row_text(app, "Fix the build")).to_contain("Completed")

	-- Renamed on another device: the same row, its new title.
	app:command("rename_elsewhere")
	app:frames(20)
	expect(`tidy @{node(app, SIDE, "button", "Tidy every test")}`).to_be(`tidy @{tidy}`)

	-- A new one arrives at the top of Atlas: the rest stay.
	app:command("arrive")
	app:frames(30)
	expect(node(app, SIDE, "button", "Brand new")).never.to_be_nil()
	expect(`fix @{node(app, SIDE, "button", "Fix the build")}`).to_be(`fix @{fix}`)
	expect(`docs @{node(app, SIDE, "button", "Write the docs")}`).to_be(`docs @{docs}`)
	expect(`atlas @{node(app, SIDE, "button", "Atlas")}`).to_be(`atlas @{atlas}`)
	expect(`borealis @{node(app, SIDE, "button", "Borealis")}`).to_be(`borealis @{borealis}`)
end)

test("a project arriving joins the list, the others' headers staying", function()
	local app = open()
	local atlas = node(app, SIDE, "button", "Atlas")
	local borealis = node(app, SIDE, "button", "Borealis")
	expect(atlas).never.to_be_nil()
	expect(borealis).never.to_be_nil()
	app:command("project_arrives")
	app:frames(30)
	expect(node(app, SIDE, "button", "Cosmos")).never.to_be_nil()
	expect(node(app, SIDE, "button", "First in Cosmos")).never.to_be_nil()
	expect(`atlas @{node(app, SIDE, "button", "Atlas")}`).to_be(`atlas @{atlas}`)
	expect(`borealis @{node(app, SIDE, "button", "Borealis")}`).to_be(`borealis @{borealis}`)
end)

test("scrolled down a long list, the sidebar holds its place as sessions change", function()
	local app = open()
	app:command("many")
	app:frames(30)
	-- (Borealis's past its first five, shown.)
	app:click(`{SIDE} >> role:button "Show 397 more"`)
	app:frames(30)
	app:scroll(`{SIDE} >> role:button "Answer the review"`, 4000)
	app:frames(30)
	local label = "Many 120"
	-- (Some row well down the list, in view now.)
	for i = 100, 300 do
		if top(app, SIDE, `Many {i}`) then
			label = `Many {i}`
			break
		end
	end
	local y = top(app, SIDE, label)
	expect(y).never.to_be_nil()
	local id = node(app, SIDE, "button", label)
	expect(id).never.to_be_nil()
	app:command("work")
	app:frames(20)
	app:command("finish")
	app:frames(20)
	expect(`{label} at {top(app, SIDE, label)}`).to_be(`{label} at {y}`)
	-- (Where it's scrolled is kept outside the sidebar: made again, it
	-- comes back there.)
	expect((app:state("shell_sidebar_offset") :: number) > 0).to_be(true)
	expect(`{label} @{node(app, SIDE, "button", label)}`).to_be(`{label} @{id}`)
end)

test("choosing a session moves the route; back and forward move the choice", function()
	local app = open()
	expect(app:state("shell_route_path")).to_be("/")
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/a1")
	expect(app:state("shell_selected")).to_be("a1")
	app:click(`{SIDE} >> role:button "Ship the release"`)
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/b1")

	-- Back, then back again to a new chat; forward to Fix the build.
	app:click('#titlebar >> role:button "Back"')
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/a1")
	expect(app:state("shell_selected")).to_be("a1")
	app:click('#titlebar >> role:button "Back"')
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/")
	expect(app:state("shell_selected")).to_be_nil()
	app:click('#titlebar >> role:button "Forward"')
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/a1")

	-- New chat, from the sidebar's top: no step back to a session chosen.
	app:click(`{SIDE} >> role:button "New chat"`)
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/")
	expect(app:state("shell_selected")).to_be_nil()

	-- Settings: from you at the foot, its pages listed.
	app:command("shell.settings")
	app:frames(20)
	expect(app:state("shell_route_path")).to_be("/settings/general")
	expect(app:has('#settings-nav >> "Account"')).to_be(true)
	app:click('#settings-nav >> "Account"')
	app:frames(20)
	expect(app:state("shell_route_path")).to_be("/settings/profile")
end)

test("the files toggle shows on a session's page only", function()
	local app = open()
	expect(app:has('#titlebar >> role:button "Panel"')).to_be(false)
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(10)
	expect(app:has('#titlebar >> role:button "Panel"')).to_be(true)
	expect(open_panel(app, "a1")).to_be(false)
	app:click('#titlebar >> role:button "Panel"')
	app:frames(30)
	expect(open_panel(app, "a1")).to_be(true)
end)

test("sessions put by hand go to the host and come back, the rows staying", function()
	local app = open()
	local docs = node(app, SIDE, "button", "Write the docs")
	local atlas = node(app, SIDE, "button", "Atlas")
	expect(atlas).never.to_be_nil()
	app:set_state("shell_places", { pinned = { "a2" }, sections = {} })
	app:frames(30)
	expect(app:state("calls")).to_contain("place:a2")
	expect(node(app, SIDE, "button", "Pinned")).never.to_be_nil()
	expect(`atlas @{node(app, SIDE, "button", "Atlas")}`).to_be(`atlas @{atlas}`)
	-- (Pinned, its row is Pinned's own: a row of another place.)
	expect(node(app, SIDE, "button", "Write the docs")).never.to_be_nil()
	expect(docs).never.to_be_nil()

	-- Archived from its corner: gone, the host told.
	app:hover(`{SIDE} >> role:button "Ship the release"`)
	app:frames(10)
	app:click(`{SIDE} >> role:button "Ship the release" >> role:button "Archive"`)
	app:frames(30)
	expect(app:state("calls")).to_contain("archive:b1")
	expect(node(app, SIDE, "button", "Ship the release")).to_be_nil()
end)

test("a screen that fails shows so in its place, and the rest goes on", function()
	local app = mount("boundary_fixture.luau")
	app:resize(800, 600)
	app:frames(5)
	expect(app:has(`#missing >> "Missing didn't load"`)).to_be(true)
	expect(app:has(`#throws >> "Throws didn't load"`)).to_be(true)
	expect(app:text("#beside")).to_be("Still here")
	-- One a fallback takes (a template's: the built-in at once) shows that,
	-- no placeholder.
	expect(app:text("#templated >> #built-in")).to_be("The built-in screen")
	expect(app:has(`#templated >> "Templated didn't load"`)).to_be(false)
	-- The screens showing, by module, for the plugin system to rebuild.
	local mounted = app:state("shell_boundary_mounted") :: any
	expect(mounted["@app/screens/shell/templated"]).to_be(1)
	expect(mounted["@app/screens/shell/nowhere"]).to_be(1)
	-- Made again from its module (once swapped): that screen alone.
	local function id(target: string): string?
		return (app:tree(target):match("@(%d+)"))
	end
	local built, beside, missing = id("#templated >> #built-in"), id("#beside"), id("#missing")
	app:command("rebuild")
	app:frames(5)
	expect(id("#templated >> #built-in")).never.to_be(built)
	expect(`beside @{id("#beside")}`).to_be(`beside @{beside}`)
	expect(`missing @{id("#missing")}`).to_be(`missing @{missing}`)
	expect((app:state("shell_boundary_mounted") :: any)["@app/screens/shell/templated"]).to_be(1)
	local failures = app:state("shell_failures") :: any
	expect(failures["@app/screens/shell/nowhere"]).never.to_be_nil()
	expect(failures["@app/screens/shell/boundary"]).to_contain("a template that throws")
end)

test("an approval asked shows on its row, in place", function()
	local app = open()
	local docs = node(app, SIDE, "button", "Write the docs")
	expect(docs).never.to_be_nil()
	app:command("ask")
	app:frames(20)
	expect(row_text(app, "Write the docs")).to_contain("Pending Approval")
	expect(`docs @{node(app, SIDE, "button", "Write the docs")}`).to_be(`docs @{docs}`)
end)

test("a section's fold goes to the host, and one folded elsewhere folds here", function()
	local app = open()
	app:command("section")
	app:frames(30)
	expect(node(app, SIDE, "button", "Answer the review")).never.to_be_nil()
	-- Folded on another device: its session goes from the list.
	app:command("fold_elsewhere")
	app:frames(40)
	expect(node(app, SIDE, "button", "Answer the review")).to_be_nil()
	-- Opened here (a click on its title): the host hears it.
	app:click(`{SIDE} >> "Later"`)
	app:frames(40)
	expect(app:state("calls")).to_contain("collapse:later:false")
	expect(node(app, SIDE, "button", "Answer the review")).never.to_be_nil()
end)

test("archived sessions list in settings, and come back", function()
	local app = open()
	app:command("shell.settings")
	app:frames(10)
	app:click('#settings-nav >> "Archived"')
	app:frames(20)
	expect(app:state("shell_route_path")).to_be("/settings/archived")
	expect(app:has('#settings-page >> "Old experiment"')).to_be(true)
	-- (Its corner's Restore, under the pointer.)
	app:hover('#settings-page >> role:group "Old experiment"')
	app:frames(20)
	app:click('#settings-page >> role:button "Restore to sidebar"')
	app:frames(30)
	expect(app:state("calls")).to_contain("unarchive:z1")
	expect(app:has('#settings-page >> "Old experiment"')).to_be(false)
	-- Back to the chats: it's in the sidebar again.
	app:click('#settings-nav >> "Back to chats"')
	app:frames(20)
	expect(node(app, SIDE, "button", "Old experiment")).never.to_be_nil()
end)

test("settings pages list under their groups' headings, a search over them", function()
	local app = open()
	app:command("grouped_pages")
	app:command("shell.settings")
	app:frames(20)
	local tree = app:tree("#settings-nav")
	expect(tree).to_contain('"Personal"')
	expect(tree).to_contain('"Devices"')
	-- (No catch-all heading once pages have groups. The foot's Settings
	-- button is a button, not a heading.)
	expect(tree).never.to_contain('text "Settings"')
	expect(app:has("#settings-nav >> #settings-search")).to_be(true)
	-- In order: a group's heading over its pages.
	local personal, profile = top_of(app, "#settings-nav", "Personal"), top_of(app, "#settings-nav", "Profile")
	local devices, mac = top_of(app, "#settings-nav", "Devices"), top_of(app, "#settings-nav", "This Mac")
	expect(personal < profile and profile < devices and devices < mac).to_be(true)
	-- The field under Back, over the pages.
	expect(top_of(app, "#settings-nav", "Back to chats") < top_of(app, "#settings-nav", "Search settings")).to_be(true)
	expect(top_of(app, "#settings-nav", "Search settings") < personal).to_be(true)
	app:click('#settings-nav >> "This Mac"')
	app:frames(20)
	expect(app:state("shell_route_path")).to_be("/settings/devices")
	-- Something typed: the pages are put away, Back and the field stay.
	app:command("settings_typed")
	app:frames(20)
	expect(app:has('#settings-nav >> "This Mac"')).to_be(false)
	expect(app:has('#settings-nav >> "Back to chats"')).to_be(true)
	expect(app:has("#settings-nav >> #settings-search")).to_be(true)
end)

test("a settings page chosen in the list opens at its top", function()
	local app = open()
	app:command("shell.settings")
	app:frames(20)
	app:resize(1200, 600)
	app:frames(10)
	-- (At a point in view: the page runs on below the short window.)
	app:scroll('#settings-page >> role:heading "General"', 400)
	app:frames(20)
	expect(app:state("shell_settings_scroll") > 0).to_be(true)
	-- (The list runs past the short window too: a taller one shows its foot,
	-- where Archived is, the page still scrolled under it.)
	app:resize(1200, 760)
	app:frames(20)
	expect(app:state("shell_settings_scroll") > 0).to_be(true)
	app:click('#settings-nav >> "Archived"')
	app:frames(20)
	expect(app:state("shell_settings_scroll")).to_be(0)
end)

test("the palette finds a session across projects and opens it", function()
	local app = open()
	app:click(`{SIDE} >> role:button "Search"`)
	app:frames(20)
	expect(app:state("shell_palette_open")).to_be(true)
	app:type('role:textbox "Search"', "Ship")
	app:frames(30)
	expect(app:has('role:option "Ship the release"')).to_be(true)
	app:key("Enter")
	app:frames(20)
	expect(app:state("shell_route_path")).to_be("/session/b1")
	expect(app:state("shell_palette_open")).to_be(false)
	-- An archived one is found too.
	app:command("shell.palette")
	app:frames(20)
	app:type('role:textbox "Search"', "Old")
	app:frames(30)
	expect(app:has('role:option "Old experiment"')).to_be(true)
end)

-- Where a node is: its top and its height, by its kind and label.
local function box(app: any, scope: string, what: string): (number?, number?)
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(what, 1, true) then
			local y, h = line:match(" %-?%d+,(%-?%d+) %d+x(%d+)")
			return tonumber(y), tonumber(h)
		end
	end
	return nil, nil
end

test("a palette section's title keeps to its row as a search moves it first", function()
	local app = open()
	app:command("shell.palette")
	app:frames(20)
	-- The first section's title, as it's made first: the room under it.
	local first_y, first_h = box(app, "#palette-list", 'text "Actions"')
	local first_row = box(app, "#palette-list", 'option "New chat"')
	expect(first_y).never.to_be_nil()
	local room = (first_row :: number) - (first_y :: number) - (first_h :: number)
	-- Settings is a later section (its title taller; maybe below the fold,
	-- as pages are added); a search for one of its rows makes it the first:
	-- its title stands as the first's.
	app:type('role:textbox "Search"', "Them")
	app:frames(30)
	local title_y, title_h = box(app, "#palette-list", 'text "Settings"')
	local row_y = box(app, "#palette-list", 'option "Theme"')
	expect(title_y).never.to_be_nil()
	expect(`room {(row_y :: number) - (title_y :: number) - (title_h :: number)}`).to_be(`room {room}`)
end)

test("settings is a page of its own, and the chats come back as they were", function()
	local app = open()
	app:command("many")
	app:frames(30)
	app:click(`{SIDE} >> role:button "Show 397 more"`)
	app:frames(30)
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(10)
	app:click('#titlebar >> role:button "Panel"')
	app:frames(30)
	app:scroll(`{SIDE} >> role:button "Answer the review"`, 1500)
	app:frames(30)
	local offset = app:state("shell_sidebar_offset")
	expect((offset :: number) > 0).to_be(true)
	-- A row in view down the list: the same node, at the same place, after.
	local label = ""
	for i = 1, 400 do
		if top(app, SIDE, `Many {i}`) then
			label = `Many {i}`
			break
		end
	end
	local row_y = top(app, SIDE, label)
	expect(row_y).never.to_be_nil()

	-- Settings over the whole window: no sidebar, composer or files; its
	-- own list, the way back at its head.
	app:command("shell.settings")
	app:frames(20)
	expect(app:state("shell_route_path")).to_be("/settings/general")
	expect(app:has("#shell-settings")).to_be(true)
	-- (The chats stay, out of sight and out of the way: kept as they are.)
	local chats = (app:tree("#shell-chats"):match("[^\n]+")) :: string
	expect(chats).to_contain(" hidden")
	expect(app:has('#settings-nav >> "Back to chats"')).to_be(true)
	-- Its pages take one step of the history between them.
	app:click('#settings-nav >> "Archived"')
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/settings/archived")

	-- Back to the chats: the session, its files, the sidebar where it was
	-- scrolled, the same nodes.
	app:click('#settings-nav >> "Back to chats"')
	app:frames(30)
	expect(app:state("shell_route_path")).to_be("/session/a1")
	expect(app:has("#shell-settings")).to_be(false)
	expect((app:tree("#shell-chats"):match("[^\n]+")) :: string).never.to_contain(" hidden")
	expect(app:state("shell_selected")).to_be("a1")
	expect(open_panel(app, "a1")).to_be(true)
	expect(app:has('#titlebar >> role:button "Panel"')).to_be(true)
	expect(app:state("shell_sidebar_offset")).to_be(offset)
	-- (Out of sight, the list held no room, so its rows in view are made
	-- again: the same rows, where they were.)
	expect(`{label} at {top(app, SIDE, label)}`).to_be(`{label} at {row_y}`)

	-- Esc goes back too, and so does a step back (⌘[).
	app:command("shell.settings")
	app:frames(20)
	app:key("Escape")
	app:frames(20)
	expect(app:state("shell_route_path")).to_be("/session/a1")
	app:command("shell.settings")
	app:frames(20)
	app:command("router.back")
	app:frames(20)
	expect(app:state("shell_route_path")).to_be("/session/a1")
	-- And forward opens settings again.
	app:command("router.forward")
	app:frames(20)
	expect(app:state("shell_route_path")).to_be("/settings/general")
end)

test("a plugin's filter narrows the sidebar in place, says so, and Show all undoes it", function()
	local app = open()
	local fix = node(app, SIDE, "button", "Fix the build")
	local atlas = node(app, SIDE, "button", "Atlas")
	expect(fix).never.to_be_nil()
	app:command("focus")
	app:frames(40)
	-- Working ones alone: the rest go, Borealis (none working) with them.
	expect(`fix @{node(app, SIDE, "button", "Fix the build")}`).to_be(`fix @{fix}`)
	expect(`atlas @{node(app, SIDE, "button", "Atlas")}`).to_be(`atlas @{atlas}`)
	expect(node(app, SIDE, "button", "Write the docs")).to_be_nil()
	expect(node(app, SIDE, "button", "Borealis")).to_be_nil()
	expect(app:text("#sidebar-filter")).to_contain("Focus is showing working sessions only")
	-- Show all: everything back, the notice gone.
	app:click('#sidebar-filter >> role:link "Show all"')
	app:frames(40)
	expect(node(app, SIDE, "button", "Write the docs")).never.to_be_nil()
	expect(node(app, SIDE, "button", "Borealis")).never.to_be_nil()
	expect(app:has("#sidebar-filter")).to_be(false)
	-- A filter of the wrong shape is none.
	app:command("focus_garbled")
	app:frames(30)
	expect(node(app, SIDE, "button", "Write the docs")).never.to_be_nil()
	expect(app:has("#sidebar-filter")).to_be(false)
end)

-- Where `target` stands across: its left, its width.
local function span(app: any, target: string): (number?, number?)
	local line = (app:tree(target):match("[^\n]+")) :: string?
	if not line then
		return nil, nil
	end
	local x, w = line:match(" (%-?%d+),%-?%d+ (%d+)x%d+")
	return tonumber(x), tonumber(w)
end

test("the title bar's strip moves the window everywhere but its controls", function()
	local app = open()
	-- (Here, that each strip is there, edge to edge, and takes a click;
	-- that a press anywhere along it moves the window, overlays and gaps
	-- between controls included, is src/shell_tests.rs's, which reads what
	-- the press asks of the window.)
	local function bare(target: string)
		expect(app:has(target)).to_be(true)
		app:click(target)
		app:frames(2)
	end
	-- Over the sidebar's head and the page, edge to edge across the window.
	bare("#sidebar-drag")
	bare("#page-drag")
	local sx, sw = span(app, "#sidebar-drag")
	local px, pw = span(app, "#page-drag")
	expect(`{sx} to {(sx :: number) + (sw :: number)}`).to_be(`0 to {px}`)
	expect((px :: number) + (pw :: number)).to_be(1200)
	-- A new chat's composer, risen over the page, leaves it bare too.
	expect(app:state("shell_route_path")).to_be("/")
	-- Over a session's files.
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(10)
	app:click('#titlebar >> role:button "Panel"')
	app:frames(40)
	bare("#panel-strip")
	bare("#page-drag")
	-- The bar's controls keep their presses (over the strip).
	app:click('#titlebar >> role:button "Panel"')
	app:frames(20)
	expect(open_panel(app, "a1")).to_be(false)
	app:click('#titlebar >> role:button "Back"')
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/")
	-- Collapsed: the page's strip runs from the window's edge.
	app:click('#titlebar >> role:button "Toggle sidebar"')
	app:frames(40)
	bare("#page-drag")
	local cx = span(app, "#page-drag")
	expect(cx).to_be(0)
	app:click('#titlebar >> role:button "Toggle sidebar"')
	app:frames(40)
	-- Settings: its list's strip and its page's.
	app:command("shell.settings")
	app:frames(20)
	bare("#settings-nav-drag")
	bare("#settings-drag")
end)

test("each session owns its panel, open or closed; a new chat shows none; the width is one", function()
	local app = open()
	local function shown(): boolean
		local _, w = span(app, "#shell-files")
		return (w or 0) > 1
	end
	-- Open in A.
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(10)
	expect(shown()).to_be(false)
	app:click('#titlebar >> role:button "Panel"')
	app:frames(40)
	expect(shown()).to_be(true)
	local _, a_width = span(app, "#shell-files")
	-- B is still closed.
	app:click(`{SIDE} >> role:button "Ship the release"`)
	app:frames(40)
	expect(open_panel(app, "b1")).to_be(false)
	expect(shown()).to_be(false)
	-- A again: open, as wide as it was.
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(40)
	expect(shown()).to_be(true)
	local _, again = span(app, "#shell-files")
	expect(again).to_be(a_width)
	-- Opened in B too: one width for both (the shipping app's).
	app:click(`{SIDE} >> role:button "Ship the release"`)
	app:frames(10)
	app:click('#titlebar >> role:button "Panel"')
	app:frames(40)
	local _, b_width = span(app, "#shell-files")
	expect(b_width).to_be(a_width)
	-- Closed in B: A keeps its own.
	app:click('#titlebar >> role:button "Panel"')
	app:frames(40)
	expect(open_panel(app, "b1")).to_be(false)
	expect(open_panel(app, "a1")).to_be(true)
	-- A new chat shows none, and its toggle isn't there.
	app:click(`{SIDE} >> role:button "New chat"`)
	app:frames(40)
	expect(shown()).to_be(false)
	expect(app:has('#titlebar >> role:button "Panel"')).to_be(false)
	-- Settings over A keeps A's open under it.
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(40)
	app:command("shell.settings")
	app:frames(20)
	expect(open_panel(app, "a1")).to_be(true)
	app:key("Escape")
	app:frames(40)
	expect(app:state("shell_route_path")).to_be("/session/a1")
	local _, w = span(app, "#shell-files")
	expect(`width {w}`).to_be(`width {a_width}`)
end)

test("the composer is made once: the sessions arriving, changing and growing leave its field", function()
	local app = open()
	local function field(): string?
		return (app:tree('role:textbox "Message"'):match("@(%d+)"))
	end
	local first = field()
	expect(first).never.to_be_nil()
	app:command("reconnect")
	app:wait('#shell-sidebar >> role:button "Fix the build"', { timeout_ms = 1000 })
	app:frames(20)
	for _, c in { "arrive", "work", "finish", "rename_elsewhere", "project_arrives", "many", "pin" } do
		app:command(c)
		app:frames(10)
	end
	expect(`field @{field()}`).to_be(`field @{first}`)
	-- A session and back, and settings over it.
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(30)
	app:command("shell.settings")
	app:frames(20)
	app:key("Escape")
	app:frames(30)
	app:click(`{SIDE} >> role:button "New chat"`)
	app:frames(30)
	expect(`field @{field()}`).to_be(`field @{first}`)
end)

test("signed out, the foot says Local; Star on GitHub shows over it until put away", function()
	local app = open()
	expect(app:has(`{SIDE} >> "Local"`)).to_be(true)
	expect(app:has(`{SIDE} >> #star-banner`)).to_be(true)
	-- (Over you at the foot.)
	expect(top_of(app, SIDE, "Star on GitHub") < top_of(app, SIDE, "Local")).to_be(true)
	app:click(`#star-banner >> role:button "Dismiss"`)
	app:frames(20)
	expect(app:has(`{SIDE} >> #star-banner`)).to_be(false)
	expect(app:state("shell_star_banner_dismissed")).to_be(true)
end)

-- A node's rectangle (its first line in the tree under `scope`): x, y, w, h.
local function rect(app: any, scope: string): (number, number, number, number)
	local tree: string = app:tree(scope)
	local line: string = tree:match("[^\n]+") or ""
	local x, y, w, h = line:match(" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
	return tonumber(x) or -1, tonumber(y) or -1, tonumber(w) or -1, tonumber(h) or -1
end

test("collapsed, the sidebar folds away entirely: no rail, the page the window's width", function()
	local app = open()
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(20)
	app:command("shell.toggle_sidebar")
	app:frames(40)
	local function alone(when: string)
		expect(app:has("#shell-rail")).to_be(false)
		local _, _, sw = rect(app, SIDE)
		expect(`{when}: sidebar {sw}`).to_be(`{when}: sidebar 0`)
		local _, _, pw = rect(app, "#shell-page")
		expect(`{when}: page {pw}`).to_be(`{when}: page 1200`)
	end
	alone("a session")
	-- Its title in the strip, clear of the bar's controls.
	local tx = rect(app, "#page-title")
	local fx, _, fw = rect(app, '#titlebar >> role:button "Forward"')
	expect(tx >= fx + fw).to_be(true)
	-- A new chat, and back: still nothing in the sidebar's place.
	app:command("shell.new_chat")
	app:frames(40)
	alone("a new chat")
	app:command("shell.toggle_sidebar")
	app:frames(40)
	local _, _, open_w = rect(app, SIDE)
	expect(open_w > 0).to_be(true)
end)

test("a launch opens the session most recently active, once the chats come; not if somewhere already", function()
	local app = open()
	-- (The fixture starts as launched, on a new chat: as a launch whose
	-- chats arrive now.)
	expect(app:state("shell_route_path")).to_be("/")
	app:set_state("screens/shell/route.luau::shell_launch_opened", false)
	app:command("arrive")
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/a4")
	expect(app:state("shell_launch_opened")).to_be(true)
	-- Once only: a new chat, then chats changing, stay where they are.
	app:command("shell.new_chat")
	app:frames(10)
	app:command("work")
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/")
	-- Somewhere already as the chats come: it stays.
	app:click(`{SIDE} >> role:button "Answer the review"`)
	app:frames(10)
	app:set_state("screens/shell/route.luau::shell_launch_opened", false)
	app:command("finish")
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/b2")
end)

test("the palette archives the session open, not one whose agent is working", function()
	local app = open()
	app:click(`{SIDE} >> role:button "Write the docs"`)
	app:frames(10)
	app:command("shell.palette")
	app:frames(20)
	app:type('role:textbox "Search"', "archive")
	app:frames(20)
	expect(node(app, "#palette-list", "option", "Archive session")).never.to_be_nil()
	app:key("Enter")
	app:frames(20)
	expect(app:state("calls")).to_contain("archive:a2")
	expect(app:state("shell_route_path")).to_be("/")
	-- Fix the build's agent is working: none offered.
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(10)
	app:command("shell.palette")
	app:frames(20)
	app:type('role:textbox "Search"', "archive")
	app:frames(20)
	expect(node(app, "#palette-list", "option", "Archive session")).to_be_nil()
end)

test("with no sessions at all, the sidebar says so; one arriving takes it away", function()
	local app = open()
	local function says(): boolean
		return app:tree("#sidebar"):find("No sessions yet", 1, true) ~= nil
	end
	-- (In one list: by project, its groups stand for themselves.)
	app:set_state("screens/shell/sidebar.luau::shell_sidebar_organize", "none")
	app:frames(20)
	expect(says()).to_be(false)
	app:command("none_left")
	app:frames(20)
	expect(says()).to_be(true)
	app:command("arrive")
	app:frames(20)
	expect(says()).to_be(false)
end)

test("the sidebar's foot says what's wrong with the connection, as production's pill does, and nothing while it's fine", function()
	local app = open()
	local function says(): string?
		local tree = app:tree("#shell-sidebar")
		for _, words in { "Changes could not be saved", "Offline — sends are saved", "Reconnecting…" } do
			if tree:find(words, 1, true) then
				return words
			end
		end
		return nil
	end
	expect(says()).to_be(nil)
	for _, case in {
		{ "connection_reconnecting", "Reconnecting…" },
		{ "connection_offline", "Offline — sends are saved" },
		{ "connection_unsaved", "Changes could not be saved" },
	} do
		app:command(case[1])
		app:frames(20)
		expect(says()).to_be(case[2])
	end
	app:command("connection_fine")
	app:frames(20)
	expect(says()).to_be(nil)
end)

test("the Archived shelf shows as asked: ten, then twenty-five more a time", function()
	local app = open()
	local function rows(): number
		local n = 0
		for line in app:tree("#sidebar"):gmatch("[^\n]+") do
			if line:find(' button "Old ', 1, true) then
				n += 1
			end
		end
		return n
	end
	expect(node(app, "#sidebar", "button", "Archived")).to_be_nil()
	app:set_state("screens/shell/sidebar.luau::shell_sidebar_show_archived", true)
	app:command("archive_many")
	app:frames(30)
	expect(node(app, "#sidebar", "button", "Archived")).never.to_be_nil()
	-- (Old experiment, then Old 1…30: the first ten, and the rest to come.)
	app:scroll("#sidebar", 4000)
	app:frames(30)
	expect(rows()).to_be(10)
	expect(node(app, "#sidebar", "button", "Show 21 more")).never.to_be_nil()
	app:click('#sidebar >> role:button "Show 21 more"')
	app:frames(30)
	app:scroll("#sidebar", 4000)
	app:frames(30)
	expect(node(app, "#sidebar", "button", "Show 21 more")).to_be_nil()
end)

test("the project filter narrows the list to one project, and put away shows them all", function()
	local app = open()
	local function has(title: string): boolean
		return node(app, "#sidebar", "button", title) ~= nil
	end
	app:set_state("screens/shell/sidebar.luau::shell_sidebar_show_project_filter", true)
	app:frames(20)
	expect(app:tree("#shell-sidebar"):find("sidebar-project-filter", 1, true) ~= nil).to_be(true)
	app:set_state("screens/shell/sidebar.luau::shell_sidebar_project", "borealis")
	app:frames(30)
	expect(has("Ship the release")).to_be(true)
	expect(has("Fix the build")).to_be(false)
	-- Put away: everything again, the filter cleared.
	app:set_state("screens/shell/sidebar.luau::shell_sidebar_show_project_filter", false)
	app:frames(30)
	expect(has("Fix the build")).to_be(true)
	expect(app:state("shell_sidebar_project")).to_be(nil)
end)

test("a session's row shows its running subagents as Zeron's pill: how many, 99+ past that, none at none", function()
	local app = open()
	local function pill(): string?
		for line in app:tree("#sidebar"):gmatch("[^\n]+") do
			local n = line:match('"(%d+) subagents? running"')
			if n then
				return n
			end
		end
		return nil
	end
	local function says(text: string): boolean
		return app:tree("#sidebar"):find(`"{text}"`, 1, true) ~= nil
	end
	expect(pill()).to_be(nil)
	app:command("subagents_3")
	app:frames(20)
	expect(pill()).to_be("3")
	expect(says("3")).to_be(true)
	app:command("subagents_120")
	app:frames(20)
	expect(says("99+")).to_be(true)
	app:command("subagents_0")
	app:frames(20)
	expect(pill()).to_be(nil)
end)

test("the session details toggle is off by default: no button, no card", function()
	local app = open()
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(20)
	expect(app:tree("#titlebar"):find("session details", 1, true)).to_be(nil)
	expect(app:has("#session-lanes")).to_be(false)
end)

test("the title bar's session details toggle shows the lanes, says which way it goes, and counts what runs", function()
	local app = open()
	-- (The card and its button are off by default: on here.)
	app:set_state("chat_lanes_enabled", true)
	app:frames(5)
	expect(app:tree("#titlebar"):find("Show session details", 1, true)).to_be(nil)
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(20)
	expect(app:tree("#titlebar"):find("Show session details", 1, true) ~= nil).to_be(true)
	app:click('#titlebar >> role:button "Show session details"')
	app:frames(20)
	expect(app:state("chat_lanes_shown")).to_be(true)
	expect(app:tree("#titlebar"):find("Hide session details", 1, true) ~= nil).to_be(true)
	-- Three subagents under it: counted on the button.
	app:command("subagents_3")
	app:frames(20)
	local count = false
	for line in app:tree("#titlebar"):gmatch("[^\n]+") do
		if line:find('text "3"', 1, true) then
			count = true
		end
	end
	expect(count).to_be(true)
end)

test("the palette finds the session's files once typing pauses, under Files, and opens one in the panel", function()
	local app = open()
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(20)
	app:command("shell.palette")
	app:frames(20)
	-- One letter: no search.
	app:type('role:textbox "Search"', "r")
	app:frames(5)
	expect(app:wait('#palette-list >> role:option "README.md"', { timeout_ms = 300 })).to_be(false)
	app:type('role:textbox "Search"', "e")
	expect(app:wait('#palette-list >> role:option "README.md"', { timeout_ms = 3000 })).to_be(true)
	expect(node(app, "#palette-list", "option", "GUIDE.md")).never.to_be_nil()
	-- (A folder isn't something it opens.)
	expect(node(app, "#palette-list", "option", "docs")).to_be_nil()
	-- Its Files scope (⌘4, as Zeron's): the files alone.
	app:key("cmd+4")
	app:frames(20)
	expect(node(app, "#palette-list", "option", "Ship the release")).to_be_nil()
	expect(node(app, "#palette-list", "option", "README.md")).never.to_be_nil()
	-- Enter: the file, in the session's panel.
	app:key("Enter")
	app:frames(40)
	expect(app:wait('#shell-files >> "README.md"', { timeout_ms = 3000 })).to_be(true)
end)

test("with nothing typed the palette lists the session's recent files, those open in its panel first", function()
	local app = open()
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(20)
	-- (Recent files head the list: its first files are theirs.)
	local function files_in_order(): { string }
		local out = {}
		for line in app:tree("#palette-list"):gmatch("[^\n]+") do
			local name = line:match(' option "([^"]+%.md)"')
			if name then
				table.insert(out, name)
			end
		end
		return out
	end
	app:command("shell.palette")
	expect(app:wait('#palette-list >> "Recent files"', { timeout_ms = 3000 })).to_be(true)
	expect(files_in_order()[1]).to_be("README.md")
	-- (Its list is there as the palette starts fading in: frames until it
	-- shows, its field with it.)
	app:frames(10)
	-- GUIDE.md opened in the panel (from the palette): it leads the next time.
	app:type('role:textbox "Search"', "gu")
	expect(app:wait('#palette-list >> role:option "GUIDE.md"', { timeout_ms = 3000 })).to_be(true)
	app:key("cmd+4")
	app:frames(10)
	app:key("Enter")
	app:frames(40)
	app:command("shell.palette")
	expect(app:wait('#palette-list >> "Recent files"', { timeout_ms = 3000 })).to_be(true)
	app:frames(10)
	expect(files_in_order()[1]).to_be("GUIDE.md")
	expect(files_in_order()[2]).to_be("README.md")
end)

test("the palette's In Files lists lines found in the session's files, with their file and line", function()
	local app = open()
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(20)
	app:command("shell.palette")
	app:frames(20)
	app:type('role:textbox "Search"', "atlas")
	expect(app:wait('#palette-list >> role:option "# Atlas"', { timeout_ms = 3000 })).to_be(true)
	local list = app:tree("#palette-list")
	expect(list:find('"In Files"', 1, true) ~= nil).to_be(true)
	expect(list:find('"README.md:1"', 1, true) ~= nil).to_be(true)
	-- (Its file names first, then what's in them.)
	expect(list:find('"Files"', 1, true) < list:find('"In Files"', 1, true)).to_be(true)
end)

-- At the sidebar's foot: you open your profile, chosen while it's open;
-- settings' own button beside you opens them, on while they're open, and a
-- press again leaves them. The settings window has the same foot.
test("the foot's you opens your profile, and its settings button toggles settings", function()
	local app = open()
	app:frames(20)
	-- Settings' button (a toggle): open, on, then off again.
	expect(app:tree("#shell-sidebar")).to_contain('checkbox "Settings"')
	app:click('#shell-sidebar >> role:checkbox "Settings"')
	app:frames(20)
	expect(app:has("#settings-nav")).to_be(true)
	expect(app:tree("#settings-nav")).to_contain('checkbox "Settings"')
	expect(app:tree("#settings-nav")).never.to_contain('"Settings" 0')
	expect(app:tree("#settings-nav")).never.to_contain('"Settings" unchecked')
	app:click('#settings-nav >> role:checkbox "Settings"')
	app:frames(20)
	expect(app:has("#settings-nav")).to_be(false)
	-- You: your profile.
	app:click('#shell-sidebar >> role:button "Local"')
	app:frames(20)
	expect(app:has("#settings-nav")).to_be(true)
	expect(app:tree()).to_contain('heading "Account"')
	-- (Signed out, the page shows the card yours will be: a headless host
	-- draws no WebP art, and says so. Nothing else may go wrong.)
	local err = app:error()
	expect(err == nil or string.find(err, "WebP is not supported", 1, true) ~= nil).to_be(true)
end)

-- Searching settings puts its results where the pages were; you and
-- settings' button stay at the list's foot, where they always are.
test("a settings search keeps the foot's you and settings at the foot", function()
	local app = open()
	app:resize(1200, 800)
	app:command("shell.settings")
	app:frames(30)
	local function y_of(label: string): number?
		for line in app:tree("#settings-nav"):gmatch("[^\n]+") do
			if line:find(`"{label}"`, 1, true) and line:find("button", 1, true) then
				return tonumber(line:match(" %-?%d+,(%-?%d+) "))
			end
		end
		return nil
	end
	local before = y_of("Local")
	expect(before ~= nil).to_be(true)
	app:type("#settings-search >> role:textbox", "theme")
	app:frames(30)
	expect(y_of("Local")).to_be(before)
	expect(app:error()).to_be_nil()
end)

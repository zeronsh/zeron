-- The sidebar's empty projects (every session they had archived): folded
-- into one row at the projects' end, "N empty projects", closed until it's
-- opened (kept with the folds); opened, each a project's header as any
-- other (its new chat, its menu). The view options' Show → Empty projects
-- off lists them nowhere. A project with none at all, just added, isn't
-- empty: it's listed with the rest, and that same option off hides it too.
-- One gaining a session leaves the fold, one losing its last joins it. On
-- the shell in ui/shell_fixture.luau.

local SIDE = "#shell-sidebar"
local FOLDED = "screens/shell/sidebar.luau::shell_sidebar_folded"
local SHOW = "screens/shell/sidebar.luau::shell_sidebar_show_empty_projects"
local ARCHIVED = "screens/shell/sidebar.luau::shell_sidebar_show_archived"
local ORGANIZE = "screens/shell/sidebar.luau::shell_sidebar_organize"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("empty_projects")
	app:frames(30)
	return app
end

local function shows(app: any, label: string): boolean
	return app:has(`{SIDE} >> role:button "{label}"`)
end

-- Where a row stands (its top), by its label.
local function top(app: any, label: string): number
	for line in app:tree(SIDE):gmatch("[^\n]+") do
		if line:find(` button "{label}"`, 1, true) then
			local y = line:match(" %-?%d+,(%-?%d+) ")
			return tonumber(y) :: number
		end
	end
	error(`no "{label}" in the sidebar`)
end

local function toggle(app: any, label: string)
	app:click(`{SIDE} >> role:button "{label}"`)
	app:frames(40)
end

test("empty projects fold into one row at the projects' end, closed until it's opened, and kept so", function()
	local app = open()
	-- Cobalt and Dune (every session archived) in the fold, closed; Ember
	-- (just added, none at all) with the rest, where its new chat starts.
	expect(shows(app, "2 empty projects")).to_be(true)
	expect(shows(app, "Cobalt")).to_be(false)
	expect(shows(app, "Dune")).to_be(false)
	expect(shows(app, "Ember")).to_be(true)
	expect(top(app, "Borealis") < top(app, "2 empty projects")).to_be(true)
	expect(top(app, "Ember") < top(app, "2 empty projects")).to_be(true)

	-- Opened: each a project's header under it, kept with the folds.
	toggle(app, "2 empty projects")
	expect(shows(app, "Cobalt")).to_be(true)
	expect(shows(app, "Dune")).to_be(true)
	expect(top(app, "2 empty projects") < top(app, "Cobalt")).to_be(true)
	expect(((app:state(FOLDED) :: any) or {})["::empty_projects"]).to_be(true)
	-- (Its sessions archived stay on the Archived shelf, not under it.)
	expect(shows(app, "Old Cobalt work")).to_be(false)

	-- Closed again.
	toggle(app, "2 empty projects")
	expect(shows(app, "Cobalt")).to_be(false)
	expect(((app:state(FOLDED) :: any) or {})["::empty_projects"]).to_be(nil)

	-- Opened and closed from outside (the folds kept, set again): it follows.
	app:set_state(FOLDED, { ["::empty_projects"] = true })
	app:frames(40)
	expect(shows(app, "Cobalt")).to_be(true)
	app:set_state(FOLDED, {})
	app:frames(40)
	expect(shows(app, "Cobalt")).to_be(false)
end)

test("an empty project's header starts a new chat in it and has its menu", function()
	local app = open()
	toggle(app, "2 empty projects")
	app:hover(`{SIDE} >> role:button "Cobalt"`)
	app:frames(20)
	app:click(`{SIDE} >> role:button "New chat in Cobalt"`)
	app:frames(30)
	expect(app:state("shell_project")).to_be("cobalt")
	expect(app:state("shell_route_path")).to_be("/")
	app:right_click(`{SIDE} >> "Dune"`)
	app:frames(20)
	expect(app:has(`role:menuitem "Edit project…"`)).to_be(true)
	expect(app:has(`role:menuitem "Remove…"`)).to_be(true)
end)

test("a project gaining a session leaves the fold; one losing its last joins it", function()
	local app = open()
	local function cobalt_bar(): string
		return app:tree(SIDE)
	end
	app:command("cobalt_arrives")
	app:frames(40)
	expect(shows(app, "1 empty project")).to_be(true)
	expect(shows(app, "Cobalt")).to_be(true)
	expect(shows(app, "Back in Cobalt")).to_be(true)
	expect(shows(app, "Dune")).to_be(false)
	expect(cobalt_bar()).never.to_contain('"2 empty projects"')

	-- Borealis's two archived (their menus' Archive): it joins the fold.
	for _, title in { "Ship the release", "Answer the review" } do
		app:right_click(`{SIDE} >> role:button "{title}"`)
		app:frames(20)
		app:click(`role:menuitem "Archive"`)
		app:frames(30)
	end
	expect(shows(app, "2 empty projects")).to_be(true)
	expect(shows(app, "Borealis")).to_be(false)
end)

test("Show → Empty projects off lists them nowhere; on, the fold is back as it was", function()
	local app = open()
	app:click(`{SIDE} >> role:button "Sidebar view options"`)
	app:frames(20)
	expect(app:has('role:checkbox "Empty projects"')).to_be(true)
	app:click('role:checkbox "Empty projects"')
	app:frames(30)
	expect(app:state(SHOW)).to_be(false)
	app:key("Escape")
	app:frames(30)
	expect(shows(app, "2 empty projects")).to_be(false)
	expect(shows(app, "Cobalt")).to_be(false)
	-- (One never used goes with them; a project with sessions stays.)
	expect(shows(app, "Ember")).to_be(false)
	expect(shows(app, "Atlas")).to_be(true)

	app:set_state(SHOW, true)
	app:frames(30)
	expect(shows(app, "2 empty projects")).to_be(true)
	expect(shows(app, "Cobalt")).to_be(false)
	-- (Back with the rest, not in the fold.)
	expect(shows(app, "Ember")).to_be(true)
end)

test("with Archived shown, an empty project stays in the fold, its sessions on the shelf", function()
	local app = open()
	app:set_state(ARCHIVED, true)
	app:frames(40)
	expect(shows(app, "2 empty projects")).to_be(true)
	expect(shows(app, "Old Cobalt work")).to_be(true)
	expect(top(app, "2 empty projects") < top(app, "Old Cobalt work")).to_be(true)
	toggle(app, "2 empty projects")
	expect(shows(app, "Cobalt")).to_be(true)
	expect(top(app, "Dune") < top(app, "Old Cobalt work")).to_be(true)
end)

test("the fold follows the grouping and the filters", function()
	local app = open()
	-- By project then device: the same fold.
	app:set_state(ORGANIZE, { "project", "device" })
	app:frames(40)
	expect(shows(app, "2 empty projects")).to_be(true)
	-- By device (then project) and none: no project's header for an empty
	-- one anywhere, so no fold.
	app:set_state(ORGANIZE, { "device", "project" })
	app:frames(40)
	expect(shows(app, "2 empty projects")).to_be(false)
	expect(shows(app, "Cobalt")).to_be(false)
	app:set_state(ORGANIZE, {})
	app:frames(40)
	expect(shows(app, "2 empty projects")).to_be(false)
	app:set_state(ORGANIZE, { "project" })
	app:frames(40)

	-- Narrowed by a plugin's filter: every group with none folds away, and
	-- the fold with them.
	app:set_state("sidebar.filter", { statuses = { "working" }, by = "Focus" })
	app:frames(40)
	expect(shows(app, "2 empty projects")).to_be(false)
	app:set_state("sidebar.filter", nil)
	app:frames(40)
	expect(shows(app, "2 empty projects")).to_be(true)

	-- The project filter narrowed to an empty one: that one stands with the
	-- rest, not folded away.
	app:set_state("screens/shell/sidebar.luau::shell_sidebar_show_project_filter", true)
	app:frames(20)
	app:set_state("screens/shell/sidebar.luau::shell_sidebar_project", "cobalt")
	app:frames(40)
	expect(shows(app, "Cobalt")).to_be(true)
	expect(shows(app, "1 empty project")).to_be(true)
end)

test("many empty projects opened: the list builds only the rows in view", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 700)
	app:frames(30)
	app:command("many_empty_projects")
	app:frames(30)
	expect(shows(app, "60 empty projects")).to_be(true)
	toggle(app, "60 empty projects")
	local function built(): number
		local n = 0
		for line in app:tree(SIDE):gmatch("[^\n]+") do
			if line:find(' button "Gone %d+"') then
				n += 1
			end
		end
		return n
	end
	expect(shows(app, "Gone 1")).to_be(true)
	expect(built() < 40).to_be(true)
	-- Scrolled to the end: the last of them, the first gone from the list.
	app:scroll("#sidebar", 4000)
	app:frames(30)
	expect(shows(app, "Gone 60")).to_be(true)
	expect(shows(app, "Gone 1")).to_be(false)
end)

test("the row reads Empty, a stack of project tiles (at most 5), and the count", function()
	-- The tiles in the row: the fixture's are letters, one each.
	local app: any
	local function stacked(label: string): number
		local n, inside, indent = 0, false, 0
		for line in app:tree(SIDE):gmatch("[^\n]+") do
			local pad = #line:match("^ *")
			if line:find(` button "{label}"`, 1, true) then
				inside, indent = true, pad
			elseif inside and pad <= indent then
				break
			elseif inside and line:match(' text "[^"%d]" ') then
				n += 1
			end
		end
		return n
	end
	app = open()
	expect(shows(app, "2 empty projects")).to_be(true)
	expect(app:has(`{SIDE} >> role:button "2 empty projects" >> "Empty"`)).to_be(true)
	expect(app:has(`{SIDE} >> role:button "2 empty projects" >> "2"`)).to_be(true)
	expect(stacked("2 empty projects")).to_be(2)

	-- Sixty: the stack stops at five, the word and the count whole.
	app = mount("shell_fixture.luau")
	app:resize(1200, 700)
	app:frames(30)
	app:command("many_empty_projects")
	app:frames(30)
	expect(shows(app, "60 empty projects")).to_be(true)
	expect(app:has(`{SIDE} >> role:button "60 empty projects" >> "Empty"`)).to_be(true)
	expect(app:has(`{SIDE} >> role:button "60 empty projects" >> "60"`)).to_be(true)
	expect(stacked("60 empty projects")).to_be(5)

	-- No chevron, as Archived has none: nothing in the row but words and tiles.
	local function block(label: string): string
		local out, inside, indent = {}, false, 0
		for line in app:tree(SIDE):gmatch("[^\n]+") do
			local pad = #line:match("^ *")
			if line:find(` button "{label}"`, 1, true) then
				inside, indent = true, pad
			elseif inside and pad <= indent then
				break
			elseif inside then
				table.insert(out, line)
			end
		end
		return table.concat(out, "\n")
	end
	expect(block("60 empty projects")).never.to_contain(" img ")
	-- Opened: the tiles have flown into the rows, none left in the header;
	-- closed, they're back in the stack.
	toggle(app, "60 empty projects")
	app:frames(40)
	expect(stacked("60 empty projects")).to_be(0)
	expect(block("60 empty projects")).never.to_contain(" img ")
	toggle(app, "60 empty projects")
	app:frames(40)
	expect(stacked("60 empty projects")).to_be(5)
end)

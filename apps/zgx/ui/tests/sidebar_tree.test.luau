-- The sidebar's Organize as a tree: by project, by device, both in either
-- order (project → device → sessions, or device → project → sessions), or
-- neither. Every setting, in both densities and with its groups folded at
-- each level: the rows in order (headers and sessions), how far in each
-- level stands, ⌘1–9's order, and each session's second line saying only
-- what its groups don't. And the view menu: the two levels toggled, and
-- reordered by their handles (dragged) or by ⌥↑ ⌥↓. On the shell in
-- ui/shell_fixture.luau, with Atlas on two devices (`atlas_on_box`).

local SIDE = "#shell-sidebar"
local K = "screens/shell/sidebar.luau::"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 1000)
	app:frames(30)
	app:command("atlas_on_box")
	app:frames(10)
	return app
end

local function set(app: any, organize: { string }, compact: boolean)
	app:set_state(`{K}shell_sidebar_organize`, organize)
	app:set_state(`{K}shell_sidebar_compact`, compact)
	-- (Nothing folded to start with.)
	app:set_state(`{K}shell_sidebar_folded`, {})
	app:frames(40)
end

-- The list's rows top to bottom (headers and sessions), and where each
-- one's first glyph or letter stands (its level's indent).
local LABELS = {
	["Atlas"] = true,
	["Borealis"] = true,
	["MacBook Pro"] = true,
	["devbox"] = true,
	["Fix the build"] = true,
	["Write the docs"] = true,
	["Tidy the tests"] = true,
	["Profile on the box"] = true,
	["Ship the release"] = true,
	["Answer the review"] = true,
}
type Row = { label: string, y: number, lead: number }
local function rows(app: any): { Row }
	local out: { Row } = {}
	local cur: Row? = nil
	for line in app:tree(SIDE):gmatch("[^\n]+") do
		local label = line:match(' button "([^"]+)"')
		if label then
			cur = nil
			if LABELS[label] then
				local y = tonumber(line:match(" %-?%d+,(%-?%d+) ")) :: number
				cur = { label = label, y = y, lead = -1 }
				table.insert(out, cur :: Row)
			end
		elseif cur and (cur :: Row).lead < 0 then
			local x = line:match(" (%-?%d+),%-?%d+ %d+x%d+")
			if x and (line:find(" img ", 1, true) or line:find(' text "', 1, true)) then
				(cur :: Row).lead = tonumber(x) :: number
			end
		end
	end
	table.sort(out, function(a, b)
		return a.y < b.y
	end)
	return out
end
local function labels(app: any): string
	local out = {}
	for _, r in rows(app) do
		table.insert(out, r.label)
	end
	return table.concat(out, ",")
end
local function lead(app: any, label: string, nth: number?): number
	local i = 0
	for _, r in rows(app) do
		if r.label == label then
			i += 1
			if i == (nth or 1) then
				return r.lead
			end
		end
	end
	return -1
end
local function order(app: any): string
	return table.concat((app:state("shell_sidebar_order") :: any) or {}, ",")
end
-- A session's second line (regular), as shown.
local function second(app: any, title: string): string
	local out = {}
	for line in app:tree(`{SIDE} >> role:button "{title}"`):gmatch("[^\n]+") do
		local words = line:match(' text "([^"]*)"')
		-- (Not its title, its corner's time, or its project tile's letter.)
		if words and not line:find(" hidden", 1, true) and words ~= title and not words:match("^%d+[smhdw]$") and words ~= "now" and not words:match("^%u$") then
			table.insert(out, words)
		end
	end
	return table.concat(out, " ")
end

-- Each setting: its rows, ⌘'s order, and Fix the build's second line
-- (on this Mac, in Atlas, on fix/build) in regular density.
local SETTINGS = {
	{
		name = "none",
		organize = {},
		rows = "Fix the build,Profile on the box,Write the docs,Tidy the tests,Ship the release,Answer the review",
		order = "a1,a5,a2,a3,b1,b2",
		second = "Atlas · MacBook Pro · fix/build",
	},
	{
		name = "project",
		organize = { "project" },
		rows = "Atlas,Fix the build,Profile on the box,Write the docs,Tidy the tests,Borealis,Ship the release,Answer the review",
		order = "a1,a5,a2,a3,b1,b2",
		second = "MacBook Pro · fix/build",
	},
	{
		name = "device",
		organize = { "device" },
		rows = "MacBook Pro,Fix the build,Write the docs,Tidy the tests,devbox,Profile on the box,Ship the release,Answer the review",
		order = "a1,a2,a3,a5,b1,b2",
		second = "Atlas · fix/build",
	},
	{
		name = "project>device",
		organize = { "project", "device" },
		rows = "Atlas,MacBook Pro,Fix the build,Write the docs,Tidy the tests,devbox,Profile on the box,Borealis,devbox,Ship the release,Answer the review",
		order = "a1,a2,a3,a5,b1,b2",
		second = "fix/build",
	},
	{
		name = "device>project",
		organize = { "device", "project" },
		rows = "MacBook Pro,Atlas,Fix the build,Write the docs,Tidy the tests,devbox,Atlas,Profile on the box,Borealis,Ship the release,Answer the review",
		order = "a1,a2,a3,a5,b1,b2",
		second = "fix/build",
	},
}

test("every Organize setting, both densities: the tree's rows, levels, ⌘ order and second lines", function()
	local app = open()
	for _, st in SETTINGS do
		for _, compact in { true, false } do
			set(app, st.organize, compact)
			local case = `{st.name} compact={compact}`
			expect(`{case}: {labels(app)}`).to_be(`{case}: {st.rows}`)
			expect(`{case} ⌘: {order(app)}`).to_be(`{case} ⌘: {st.order}`)
			if not compact then
				expect(`{case} second: {second(app, "Fix the build")}`).to_be(`{case} second: {st.second}`)
			end
			-- Each level a step further in: an inner group's tile past its
			-- outer group's, its sessions past both.
			if #st.organize == 2 then
				local outer = if st.organize[1] == "project" then "Atlas" else "MacBook Pro"
				local inner = if st.organize[1] == "project" then "MacBook Pro" else "Atlas"
				expect(`{case}: inner {lead(app, inner) > lead(app, outer)}`).to_be(`{case}: inner true`)
				expect(`{case}: rows {lead(app, "Fix the build") > lead(app, inner)}`).to_be(`{case}: rows true`)
			elseif #st.organize == 1 then
				local outer = if st.organize[1] == "project" then "Atlas" else "MacBook Pro"
				expect(`{case}: rows {lead(app, "Fix the build") > lead(app, outer)}`).to_be(`{case}: rows true`)
			end
		end
	end
end)

test("folding at each level: an outer group's inner groups and sessions go, an inner group's sessions; working ones stay while kept", function()
	local app = open()
	-- Project → device: Atlas folded. Fix the build works, so it stays,
	-- under its inner group's header (Active when collapsed, on).
	set(app, { "project", "device" }, true)
	app:click(`{SIDE} >> role:button "Atlas"`)
	app:frames(40)
	expect(labels(app)).to_be("Atlas,MacBook Pro,Fix the build,Borealis,devbox,Ship the release,Answer the review")
	expect(order(app)).to_be("a1,b1,b2")
	-- Atlas open again, its MacBook Pro folded: only that group's idle
	-- sessions go.
	app:click(`{SIDE} >> role:button "Atlas"`)
	app:frames(40)
	app:click(`{SIDE} >> role:button "MacBook Pro"`)
	app:frames(40)
	expect(labels(app)).to_be("Atlas,MacBook Pro,Fix the build,devbox,Profile on the box,Borealis,devbox,Ship the release,Answer the review")
	expect(order(app)).to_be("a1,a5,b1,b2")
	-- Folds kept nothing in a state: whole groups go.
	app:set_state(`{K}shell_sidebar_keep_active`, false)
	app:frames(40)
	expect(labels(app)).to_be("Atlas,MacBook Pro,devbox,Profile on the box,Borealis,devbox,Ship the release,Answer the review")
	expect(order(app)).to_be("a5,b1,b2")
	app:set_state(`{K}shell_sidebar_keep_active`, true)

	-- Device → project: devbox folded. Ship the release waits on
	-- you, so it stays under Borealis's header; Atlas's there goes.
	set(app, { "device", "project" }, true)
	app:click(`{SIDE} >> role:button "devbox"`)
	app:frames(40)
	expect(labels(app)).to_be("MacBook Pro,Atlas,Fix the build,Write the docs,Tidy the tests,devbox,Borealis,Ship the release")
	expect(order(app)).to_be("a1,a2,a3,b1")
end)

test("Show N more is per innermost group", function()
	local app = open()
	app:command("crowd_atlas")
	app:frames(10)
	set(app, { "project", "device" }, true)
	-- Atlas on this Mac holds nine: its first five (and the working one),
	-- then Show 4 more; on devbox, its one.
	expect(app:has(`{SIDE} >> role:button "Show 4 more"`)).to_be(true)
	expect(app:has(`{SIDE} >> role:button "Atlas 3"`)).to_be(false)
	expect(app:has(`{SIDE} >> role:button "Profile on the box"`)).to_be(true)
end)

-- The view menu's Organize rows, top to bottom (their labels).
local function menu_rows(app: any): string
	local out: { { y: number, l: string } } = {}
	for line in app:tree():gmatch("[^\n]+") do
		local l = line:match(' checkbox "(By [a-z]+)"')
		if l then
			table.insert(out, { y = tonumber(line:match(" %-?%d+,(%-?%d+) ")) :: number, l = l })
		end
	end
	table.sort(out, function(a, b)
		return a.y < b.y
	end)
	local names = {}
	for _, r in out do
		table.insert(names, r.l)
	end
	return table.concat(names, ",")
end
local function organize(app: any): string
	return table.concat((app:state(`{K}shell_sidebar_organize`) :: any) or {}, ",")
end

test("the view menu: both levels on at once, None off; their handles reorder the tree, dragged or with ⌥↑ ⌥↓", function()
	local app = open()
	set(app, { "project" }, true)
	app:click('role:button "Sidebar view options"')
	app:frames(20)
	expect(menu_rows(app)).to_be("By project,By device")
	-- Each level's grip stands in its glyph's place: the same column as
	-- None's icon, its name where None's starts.
	local function glyph_x(label: string): (number?, number?)
		local tree = app:tree(`role:checkbox "{label}"`)
		local img = tree:match(" img (%-?%d+),")
		local row = tree:match(" (%-?%d+),%-?%d+ %d+x%d+")
		return tonumber(img), tonumber(row)
	end
	local nx, nrow = glyph_x("None")
	local px, prow = glyph_x("By project")
	local dx, drow = glyph_x("By device")
	expect(`{(px :: number) - (prow :: number)} {(dx :: number) - (drow :: number)}`).to_be(`{(nx :: number) - (nrow :: number)} {(nx :: number) - (nrow :: number)}`)
	-- Both on: project, then device; the menu stays open.
	app:click('role:checkbox "By device"')
	app:frames(20)
	expect(organize(app)).to_be("project,device")
	expect(labels(app):sub(1, 18)).to_be("Atlas,MacBook Pro,")
	-- Dragged by its handle above By project: device, then project.
	app:drag('role:checkbox "By device"', 'role:checkbox "By project"')
	app:frames(30)
	expect(menu_rows(app)).to_be("By device,By project")
	expect(organize(app)).to_be("device,project")
	expect(labels(app):sub(1, 18)).to_be("MacBook Pro,Atlas,")
	-- Off and on again: it keeps its place.
	app:click('role:checkbox "By device"')
	app:frames(10)
	expect(organize(app)).to_be("project")
	app:click('role:checkbox "By device"')
	app:frames(10)
	expect(organize(app)).to_be("device,project")
	-- ⌥↓ on it (focused, clicked): down a place.
	app:key("alt+ArrowDown")
	app:frames(30)
	expect(menu_rows(app)).to_be("By project,By device")
	expect(organize(app)).to_be("project,device")
	-- ⌥↑: back up.
	app:key("alt+ArrowUp")
	app:frames(30)
	expect(organize(app)).to_be("device,project")
	-- None: both off; the rows stay in their order.
	app:click('role:checkbox "None"')
	app:frames(20)
	expect(organize(app)).to_be("")
	expect(menu_rows(app)).to_be("By device,By project")
	expect(app:has('role:checkbox "By project"')).to_be(true)
end)

test("Organize kept as one choice before reads as its one level", function()
	local app = open()
	app:set_state(`{K}shell_sidebar_organize`, "device")
	app:frames(20)
	expect(organize(app)).to_be("device")
	expect(labels(app):sub(1, 12)).to_be("MacBook Pro,")
end)

test("with both levels on, pinning a session takes it out of its group", function()
	local app = open()
	set(app, { "project", "device" }, true)
	-- Pinned (as a drop on Pinned puts it): out of Atlas › MacBook Pro.
	app:set_state("shell_places", { pinned = { "a2" }, sections = {} })
	app:frames(40)
	expect(rows(app)[1].label).to_be("Write the docs")
	expect(order(app)).to_be("a2,a1,a3,a5,b1,b2")
	expect(labels(app)).to_be("Write the docs,Atlas,MacBook Pro,Fix the build,Tidy the tests,devbox,Profile on the box,Borealis,devbox,Ship the release,Answer the review")
end)

test("with both levels on, the rail's tiles are the outer groups and a flyout holds all of its sessions", function()
	-- (The design system's sidebar and its rail, ui/sidebar_fixture.luau:
	-- Borealis on Mac and Box.)
	local app = mount("sidebar_fixture.luau")
	app:resize(900, 700)
	app:frames(30)
	app:set_state("test_sidebar_organize", { "project", "device" })
	app:frames(40)
	-- Open: Borealis › Mac and Box.
	expect(app:has('#wide >> role:button "Box"')).to_be(true)
	-- The rail: its outer groups' tiles alone.
	expect(app:has('#rail >> role:button "Atlas"')).to_be(true)
	expect(app:has('#rail >> role:button "Borealis"')).to_be(true)
	expect(app:has('#rail >> role:button "Box"')).to_be(false)
	app:click('#rail >> role:button "Borealis"')
	app:frames(20)
	expect(app:tree("#rail"):find('button "Ship the release"', 1, true) ~= nil).to_be(true)
	expect(app:tree("#rail"):find('button "Answer the review"', 1, true) ~= nil).to_be(true)
end)

-- Whether the sidebar shows a row (a header or a session), by its label.
local function shows(app: any, label: string): boolean
	return app:has(`{SIDE} >> role:button "{label}"`)
end

test("an outer group pages its inner groups as sessions are paged: its first five, then Show N more, Show less, remembered", function()
	local app = open()
	app:command("many_projects")
	app:frames(10)
	set(app, { "device", "project" }, true)
	-- MacBook Pro: Atlas and Proj 1–4, then Show 2 more (Proj 5, 6).
	expect(shows(app, "Proj 4")).to_be(true)
	expect(shows(app, "Proj 5")).to_be(false)
	expect(shows(app, "Proj 6")).to_be(false)
	expect(shows(app, "Show 2 more")).to_be(true)
	-- (⌘1–9 go by what shows: Proj 5's and 6's sessions aren't in it.)
	expect(order(app):find("q5", 1, true)).to_be_nil()
	-- Shown: all seven, and Show less; kept for the session (its key, the
	-- outer group's), through a change of grouping and back.
	app:click(`{SIDE} >> role:button "Show 2 more"`)
	app:frames(40)
	expect(shows(app, "Proj 6")).to_be(true)
	expect(shows(app, "Show less")).to_be(true)
	expect(order(app):find("q6", 1, true) ~= nil).to_be(true)
	expect(((app:state("shell_sidebar_expanded") :: any) or {})["device:MacBook Pro/groups"]).to_be(true)
	app:set_state(`{K}shell_sidebar_organize`, { "project" })
	app:frames(40)
	app:set_state(`{K}shell_sidebar_organize`, { "device", "project" })
	app:frames(40)
	expect(shows(app, "Proj 6")).to_be(true)
	-- Show less: five again.
	app:click(`{SIDE} >> role:button "Show less"`)
	app:frames(40)
	expect(shows(app, "Proj 6")).to_be(false)
	expect(shows(app, "Show 2 more")).to_be(true)
end)

test("an inner group with a session in a state, or the session open, shows past the first five", function()
	local app = open()
	app:command("many_projects")
	app:frames(10)
	set(app, { "device", "project" }, true)
	-- Proj 6's session waits on you: its group shows; one left behind.
	app:command("proj6_asks")
	app:frames(30)
	expect(shows(app, "Proj 6")).to_be(true)
	expect(shows(app, "Proj 5")).to_be(false)
	expect(shows(app, "Show 1 more")).to_be(true)
	-- Proj 5's session open: its group shows too; none left behind.
	app:set_state("shell_selected", "q5")
	app:frames(30)
	expect(shows(app, "Proj 5")).to_be(true)
	expect(shows(app, "Show 1 more")).to_be(false)
end)

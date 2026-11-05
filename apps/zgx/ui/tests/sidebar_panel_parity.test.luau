-- The panel home's side chats are drawn as the sidebar draws sessions, for
-- every one of its view options: each option on and off, in both
-- densities and each way of grouping, a side chat's row matches a sidebar
-- row with the same data (Tidy the tests: on this computer, in Atlas, no
-- branch), and its group's header matches the sidebar's: height, where
-- its title starts (its mark, its project's tile, its tree's gutter), its
-- second line's words, its header's tile. On the shell in
-- ui/shell_fixture.luau, the sidebar's view as the shell publishes it.

local SIDE = "#shell-sidebar"
local PANEL = "#panel-home-side-list"
local KEY = "screens/shell/sidebar.luau::"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1400, 1000)
	app:frames(30)
	app:command("side_chats")
	app:frames(5)
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(20)
	app:click('#titlebar >> role:button "Panel"')
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	expect(app:wait(PANEL, { timeout_ms = 15000 })).to_be(true)
	app:frames(30)
	return app
end

-- A row as the tree draws it, by its label under `scope`: its height, and
-- what's in it, each where it stands from the row's left edge: its glyphs
-- (`img`) and its words, all but its title and its corner's time (what
-- differs between two sessions). (A sidebar row's title is its label, which
-- the tree doesn't repeat; where its words start shows in its second line.)
type Row = { h: number, inside: string }
local function row(app: any, scope: string, label: string, title: string?): Row?
	local tree = app:tree(`{scope} >> role:button "{label}"`)
	local h: number? = nil
	local left = 0
	local parts: { string } = {}
	for line in tree:gmatch("[^\n]+") do
		local x, w, hh = line:match(" (%-?%d+),%-?%d+ (%d+)x(%d+)")
		if h == nil then
			if x then
				h, left = tonumber(hh), tonumber(x) :: number
			end
		elseif x and (tonumber(w) :: number) > 0 then
			local at = (tonumber(x) :: number) - left
			local words = line:match(' text "([^"]*)"')
			local hidden = line:find(" hidden", 1, true) ~= nil
			if words then
				local time = words == "now" or words:match("^%d+[smhdw]$") ~= nil
				if words ~= title and not time and not hidden then
					table.insert(parts, `"{words}"@{at}`)
				end
			elseif line:find(" img ", 1, true) then
				table.insert(parts, `img@{at}`)
			end
		end
	end
	if h == nil then
		return nil
	end
	table.sort(parts)
	return { h = h :: number, inside = table.concat(parts, " ") }
end

local OPTIONS = { "harness", "project_icon", "location", "branch" }

local function set(app: any, compact: boolean, organize: any, off: string?)
	app:set_state(`{KEY}shell_sidebar_compact`, compact)
	app:set_state(`{KEY}shell_sidebar_organize`, organize)
	for _, o in OPTIONS do
		app:set_state(`{KEY}shell_sidebar_show_{o}`, o ~= off)
	end
	app:frames(40)
end

test("every sidebar view option draws a side chat's row and group header as the sidebar's", function()
	local app = open()
	local checked = 0
	for _, compact in { true, false } do
		-- (Organize's every setting: one level, either, both in either
		-- order, none.)
		for _, organize in { { "project" }, { "device" }, {}, { "project", "device" }, { "device", "project" } } do
			for _, off in { "", "harness", "project_icon", "location", "branch" } do
				set(app, compact, organize, if off == "" then nil else off)
				local case = `compact={compact} organize={table.concat(organize, ">")} off={off}`
				local s = row(app, SIDE, "Tidy the tests")
				local p = row(app, PANEL, "Open side chat Look into the flake", "Look into the flake")
				expect(`{case}: rows {s ~= nil} {p ~= nil}`).to_be(`{case}: rows true true`)
				local sr, pr = s :: Row, p :: Row
				-- Height: the same density, the same second line.
				expect(`{case} height {pr.h}`).to_be(`{case} height {sr.h}`)
				-- What it shows, where: its mark, its project's tile, its
				-- second line's words (where its title's words start).
				expect(`{case}: {pr.inside}`).to_be(`{case}: {sr.inside}`)
				-- Its groups' headers at each level: there or not, their tiles
				-- shown or not, an inner one a step in.
				local NAME = { project = "Atlas", device = "MacBook Pro" }
				for _, level in organize do
					local group = NAME[level]
					local sh = row(app, SIDE, group) :: Row
					local ph = row(app, PANEL, group)
					expect(`{case} {group} header {ph ~= nil}`).to_be(`{case} {group} header true`)
					expect(`{case} {group} header: {(ph :: Row).inside} {(ph :: Row).h}`).to_be(`{case} {group} header: {sh.inside} {sh.h}`)
				end
				if #organize == 0 then
					expect(`{case} {app:has(`{PANEL} >> role:button "Atlas"`)}`).to_be(`{case} false`)
				end
				checked += 1
			end
		end
	end
	expect(checked).to_be(50)
end)

test("side chats follow the sidebar's sort: last updated, or created", function()
	local app = open()
	set(app, true, {}, nil)
	local function first(): string
		local a = row(app, PANEL, "Open side chat Look into the flake") :: Row
		local b = row(app, PANEL, "Open side chat Write the changelog") :: Row
		local ay = app:tree(`{PANEL} >> role:button "Open side chat Look into the flake"`):match(" %-?%d+,(%-?%d+) ")
		local by = app:tree(`{PANEL} >> role:button "Open side chat Write the changelog"`):match(" %-?%d+,(%-?%d+) ")
		local _ = a and b
		return if tonumber(ay) < tonumber(by) then "flake" else "changelog"
	end
	-- Last updated: the flake (moved 30 s ago) first.
	app:set_state(`{KEY}shell_sidebar_sort`, "updated")
	app:frames(30)
	expect(first()).to_be("flake")
	-- Created: the changelog (made an hour ago, the flake five) first.
	app:set_state(`{KEY}shell_sidebar_sort`, "created")
	app:frames(30)
	expect(first()).to_be("changelog")
end)

test("a side chat's row archives from its corner under the pointer, as a sidebar row does", function()
	local app = open()
	app:hover(`{PANEL} >> role:button "Open side chat Write the changelog"`)
	app:frames(10)
	expect(app:has(`{PANEL} >> role:button "Archive"`)).to_be(true)
	app:click(`{PANEL} >> role:button "Archive"`)
	app:frames(10)
	expect(tostring(app:state("calls") or "")).to_contain("archive:s2")
end)

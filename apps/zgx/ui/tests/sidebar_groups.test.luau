-- The sidebar's groups: the most recently active first (by project and by
-- device), each showing its first five sessions then "Show N more" (the
-- session open showing past them, ⌘1–9 and Ctrl-Tab going by what shows),
-- and a folded group still listing its sessions in a state while
-- "Active when collapsed" is on. On the shell in ui/shell_fixture.luau.

local SIDE = "#shell-sidebar"
local ORGANIZE = "screens/shell/sidebar.luau::shell_sidebar_organize"
local KEEP = "screens/shell/sidebar.luau::shell_sidebar_keep_active"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	return app
end

-- A node's identity under the sidebar, by its kind and label: `@id`.
local function node(app: any, kind: string, label: string): string?
	for line in app:tree(SIDE):gmatch("[^\n]+") do
		if line:find(` {kind} "{label}"`, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
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

local function shows(app: any, label: string): boolean
	return app:has(`{SIDE} >> role:button "{label}"`)
end

local function order(app: any): string
	return table.concat((app:state("shell_sidebar_order") :: any) or {}, ",")
end

test("groups stand the most recently active first, by project and by device", function()
	local app = open()
	local atlas = node(app, "button", "Atlas")
	local borealis = node(app, "button", "Borealis")
	expect(top(app, "Atlas") < top(app, "Borealis")).to_be(true)

	-- Borealis's oldest moves: Borealis goes first, both headers the same
	-- nodes.
	app:command("borealis_moves")
	app:frames(30)
	expect(top(app, "Borealis") < top(app, "Atlas")).to_be(true)
	expect(`atlas @{node(app, "button", "Atlas")}`).to_be(`atlas @{atlas}`)
	expect(`borealis @{node(app, "button", "Borealis")}`).to_be(`borealis @{borealis}`)
	-- (⌘1 follows: Borealis's newest first.)
	expect(order(app)).to_be("b2,b1,a1,a2,a3")

	-- By device: devbox (Borealis's) is the most recent, ahead of
	-- this Mac.
	app:set_state(ORGANIZE, "device")
	app:frames(40)
	expect(top(app, "devbox") < top(app, "MacBook Pro")).to_be(true)
end)

test("a long group shows five, then Show N more and Show less; the session open shows past them", function()
	local app = open()
	app:command("crowd_atlas")
	app:frames(30)
	-- Atlas: Fix the build, Write the docs, Tidy the tests, Atlas 1 and 2.
	expect(shows(app, "Atlas 2")).to_be(true)
	expect(shows(app, "Atlas 3")).to_be(false)
	expect(shows(app, "Show 4 more")).to_be(true)
	local pager = node(app, "button", "Show 4 more")

	-- Atlas 5 opened (from search, say): it shows under the five, the
	-- rest still folded.
	app:set_state("shell_selected", "x5")
	app:frames(30)
	expect(shows(app, "Atlas 5")).to_be(true)
	expect(shows(app, "Atlas 4")).to_be(false)
	expect(shows(app, "Show 3 more")).to_be(true)
	expect(top(app, "Atlas 2") < top(app, "Atlas 5")).to_be(true)
	expect(top(app, "Atlas 5") < top(app, "Show 3 more")).to_be(true)
	-- (The same row, its count followed.)
	expect(`pager @{node(app, "button", "Show 3 more")}`).to_be(`pager @{pager}`)

	-- Shown: all of them, in their order, then Show less; kept for the
	-- session.
	local five = node(app, "button", "Atlas 5")
	app:click(`{SIDE} >> role:button "Show 3 more"`)
	app:frames(40)
	expect(shows(app, "Atlas 3")).to_be(true)
	expect(shows(app, "Atlas 6")).to_be(true)
	expect(top(app, "Atlas 4") < top(app, "Atlas 5")).to_be(true)
	expect(shows(app, "Show less")).to_be(true)
	expect(`five @{node(app, "button", "Atlas 5")}`).to_be(`five @{five}`)
	expect(((app:state("shell_sidebar_expanded") :: any) or {})["project:atlas"]).to_be(true)

	-- Show less: back to the five and the one open.
	app:click(`{SIDE} >> role:button "Show less"`)
	app:frames(40)
	expect(shows(app, "Atlas 3")).to_be(false)
	expect(shows(app, "Atlas 5")).to_be(true)
	expect(shows(app, "Show 3 more")).to_be(true)
end)

test("⌘1–9 and Ctrl-Tab go by the sessions that show", function()
	local app = open()
	app:command("crowd_atlas")
	app:frames(30)
	-- Atlas's five, then Borealis's two: Atlas 3–6 aren't in it. (Their
	-- commands, as the menu bar's keys run them.)
	expect(order(app)).to_be("a1,a2,a3,x1,x2,b1,b2")
	app:command("shell.session_6")
	app:frames(20)
	expect(app:state("shell_route_path")).to_be("/session/b1")
	-- From Atlas 2, the last that shows in Atlas, the next is Borealis's.
	app:command("shell.session_5")
	app:frames(20)
	expect(app:state("shell_route_path")).to_be("/session/x2")
	app:command("shell.next_session")
	app:frames(20)
	expect(app:state("shell_route_path")).to_be("/session/b1")
	-- Shown: they're in it, in their places.
	app:click(`{SIDE} >> role:button "Show 4 more"`)
	app:frames(40)
	expect(order(app)).to_be("a1,a2,a3,x1,x2,x3,x4,x5,x6,b1,b2")
end)

test("a collapsed group still lists its sessions in a state, each leaving as it settles", function()
	local app = open()
	app:command("crowd_atlas")
	app:command("atlas_asks")
	app:frames(30)
	-- Waiting on you, Atlas 6 shows past the five already.
	expect(shows(app, "Atlas 6")).to_be(true)
	expect(shows(app, "Show 3 more")).to_be(true)
	local fix = node(app, "button", "Fix the build")
	local six = node(app, "button", "Atlas 6")

	-- Atlas folded: Fix the build (working) and Atlas 6 (asking) stay,
	-- the same rows; the idle ones go, and so does Show more.
	app:click(`{SIDE} >> role:button "Atlas"`)
	app:frames(60)
	expect(shows(app, "Write the docs")).to_be(false)
	expect(shows(app, "Atlas 1")).to_be(false)
	expect(shows(app, "Show 3 more")).to_be(false)
	expect(`fix @{node(app, "button", "Fix the build")}`).to_be(`fix @{fix}`)
	expect(`six @{node(app, "button", "Atlas 6")}`).to_be(`six @{six}`)
	expect(order(app)).to_be("a1,x6,b1,b2")

	-- Answered: it leaves.
	app:command("atlas_answered")
	app:frames(30)
	expect(shows(app, "Atlas 6")).to_be(false)
	expect(shows(app, "Fix the build")).to_be(true)

	-- Turned off (the view options' "Active when collapsed"): a folded
	-- group hides them all.
	app:click(`{SIDE} >> role:button "Sidebar view options"`)
	app:frames(20)
	expect(app:has('role:menuitem "Active when collapsed"') or app:has('"Active when collapsed"')).to_be(true)
	app:key("Escape")
	app:frames(10)
	app:set_state(KEEP, false)
	app:frames(30)
	expect(shows(app, "Fix the build")).to_be(false)
	expect(order(app)).to_be("b1,b2")
end)

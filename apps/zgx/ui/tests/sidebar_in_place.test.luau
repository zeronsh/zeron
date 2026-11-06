-- The sidebar follows its places in place: a session pinned, a section made
-- and renamed, leave every tile on the rail and every header in the open
-- list the same node, and a flyout open on the rail open. (The rail was
-- built again for any change of places, which closed the flyout, and a
-- header was keyed by how many it held, so it was built again as a session
-- left it.) On the sidebars in ui/sidebar_fixture.luau.

-- A node's identity, by its kind and label, under `scope`: `@id`.
local function node(app: any, scope: string, kind: string, label: string): string?
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(` {kind} "{label}"`, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

-- Whether the rail (its flyouts included) shows a session's row.
local function on_rail(app: any, title: string): boolean
	return app:tree("#rail"):find(`button "{title}"`, 1, true) ~= nil
end

local function settle(app: any)
	app:frames(30)
end

local function places(pinned: { string }, sections: { any }): any
	return { pinned = pinned, sections = sections }
end

test("the rail's tiles and an open flyout stay as sessions are pinned and sections renamed", function()
	local app = mount("sidebar_fixture.luau")
	app:resize(900, 700)
	settle(app)
	local atlas = node(app, "#rail", "button", "Atlas")
	local borealis = node(app, "#rail", "button", "Borealis")
	expect(atlas).never.to_be_nil()

	-- Atlas's flyout, open.
	app:click('#rail >> role:button "Atlas"')
	settle(app)
	expect(on_rail(app, "Write the docs")).to_be(true)

	-- One of its sessions pinned: a Pinned tile comes, Atlas's stays, and
	-- its flyout stays open, the session gone from it.
	app:set_state("test_sidebar_places", places({ "a2" }, {}))
	settle(app)
	expect(node(app, "#rail", "button", "Pinned")).never.to_be_nil()
	expect(`Atlas @{node(app, "#rail", "button", "Atlas")}`).to_be(`Atlas @{atlas}`)
	expect(`Borealis @{node(app, "#rail", "button", "Borealis")}`).to_be(`Borealis @{borealis}`)
	expect(on_rail(app, "Fix the build")).to_be(true)
	expect(on_rail(app, "Write the docs")).to_be(false)

	-- A section of your own, then renamed: its tile is the same node, its
	-- name the new one.
	app:set_state("test_sidebar_places", places({ "a2" }, { { id = "s1", name = "Later", sessions = { "b2" } } }))
	settle(app)
	local later = node(app, "#rail", "button", "Later")
	expect(later).never.to_be_nil()
	app:set_state("test_sidebar_places", places({ "a2" }, { { id = "s1", name = "Someday", sessions = { "b2" } } }))
	settle(app)
	expect(`section @{node(app, "#rail", "button", "Someday")}`).to_be(`section @{later}`)
	expect(`Atlas @{node(app, "#rail", "button", "Atlas")}`).to_be(`Atlas @{atlas}`)
	expect(`Borealis @{node(app, "#rail", "button", "Borealis")}`).to_be(`Borealis @{borealis}`)
	expect(on_rail(app, "Fix the build")).to_be(true)
end)

test("the open list's headers stay as their sessions come and go", function()
	local app = mount("sidebar_fixture.luau")
	app:resize(900, 700)
	settle(app)
	local atlas = node(app, "#wide", "button", "Atlas")
	local borealis = node(app, "#wide", "button", "Borealis")
	local title = node(app, "#wide", "button", "Projects")
	expect(atlas).never.to_be_nil()

	-- A session pinned out of Atlas, one moved into a section out of
	-- Borealis, the section renamed.
	app:set_state("test_sidebar_places", places({ "a2" }, {}))
	settle(app)
	expect(node(app, "#wide", "button", "Pinned")).never.to_be_nil()
	expect(`Atlas @{node(app, "#wide", "button", "Atlas")}`).to_be(`Atlas @{atlas}`)
	app:set_state("test_sidebar_places", places({ "a2" }, { { id = "s1", name = "Later", sessions = { "b2" } } }))
	settle(app)
	expect(`Borealis @{node(app, "#wide", "button", "Borealis")}`).to_be(`Borealis @{borealis}`)
	expect(`Projects @{node(app, "#wide", "button", "Projects")}`).to_be(`Projects @{title}`)
	local later = node(app, "#wide", "button", "Later")
	expect(later).never.to_be_nil()
	app:set_state("test_sidebar_places", places({ "a2", "a3" }, { { id = "s1", name = "Someday", sessions = { "b2" } } }))
	settle(app)
	expect(`section @{node(app, "#wide", "button", "Someday")}`).to_be(`section @{later}`)
	expect(`Atlas @{node(app, "#wide", "button", "Atlas")}`).to_be(`Atlas @{atlas}`)
	expect(`Borealis @{node(app, "#wide", "button", "Borealis")}`).to_be(`Borealis @{borealis}`)
end)

-- A folded section says how many it holds on one line down the sidebar, a
-- section of yours (its "…" stands a step further out than words do) as
-- Pinned and Projects do.
test("every folded section's count ends on the same line", function()
	local app = mount("sidebar_fixture.luau")
	app:resize(900, 700)
	settle(app)
	app:set_state("test_sidebar_places", places({ "a2" }, { { id = "s1", name = "Later", sessions = { "b2" } } }))
	settle(app)
	local scope = "#wide"
	for _, title in { "Pinned", "Later", "Projects" } do
		app:click(`{scope} >> "{title}"`)
		settle(app)
	end
	-- The pointer away: under it, a section of yours shows its "…" instead.
	app:hover("#rail")
	settle(app)
	local ends: { number } = {}
	for line in app:tree(scope):gmatch("[^\n]+") do
		local x, w = line:match('text "%d+" (%-?[%d%.]+),%-?[%d%.]+ ([%d%.]+)x')
		if x and w then
			table.insert(ends, tonumber(x) + tonumber(w))
		end
	end
	expect(#ends).to_be(3)
	expect(ends[1]).to_be(ends[2])
	expect(ends[2]).to_be(ends[3])
	-- And their names start on one line, each row as tall.
	local starts: { [string]: number } = {}
	for line in app:tree(scope):gmatch("[^\n]+") do
		for _, title in { "Pinned", "Later", "Projects" } do
			local x = line:match(`text "{title}" (%-?[%d%.]+),`)
			if x then
				starts[title] = tonumber(x) :: number
			end
		end
	end
	expect(starts.Later).to_be(starts.Pinned)
	expect(starts.Later).to_be(starts.Projects)
end)

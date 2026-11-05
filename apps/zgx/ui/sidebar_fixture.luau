-- The sidebar, open (#wide) and as its rail (#rail), sharing one set of
-- places, for tests/sidebar_in_place.test.luau: sessions pinned, moved
-- into a section and a section renamed leave the tiles and headers the
-- same nodes. Here, beside app.luau, as a mounted script loads modules
-- from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()

local places = ui.state("test_sidebar_places", { pinned = {}, sections = {} } :: any)
local selected = ui.state("test_sidebar_selected", nil :: string?)
-- How both group (tests/sidebar_tree: both levels, the rail's tiles).
local organize = ui.state("test_sidebar_organize", { "project" } :: any)

local projects = {
	{
		id = "atlas",
		name = "Atlas",
		hue = "violet",
		sessions = {
			{ id = "a1", title = "Fix the build", status = "working", time = "5m", device = "Mac" },
			{ id = "a2", title = "Write the docs", time = "1h", device = "Mac" },
			{ id = "a3", title = "Tidy the tests", time = "2h", device = "Mac" },
		},
	},
	{
		id = "borealis",
		name = "Borealis",
		hue = "blue",
		sessions = {
			{ id = "b1", title = "Ship the release", status = "input", time = "3h", device = "Mac" },
			{ id = "b2", title = "Answer the review", time = "1d", device = "Box" },
		},
	},
}

local function sidebar(rail: boolean): Node
	return ds.Sidebar {
		compact = ui.state(`test_sidebar_compact_{rail}`, true),
		collapsed = ui.state(`test_sidebar_collapsed_{rail}`, rail),
		island = rail,
		selected = selected,
		places = places,
		organize = organize,
		projects = projects :: any,
		here = "Mac",
	}
end

return ui.row {
	w_full = true,
	h_full = true,
	ui.column { id = "wide", w = 280, h_full = true, sidebar(false) },
	ui.column { id = "rail", w = 80, h_full = true, sidebar(true) },
}

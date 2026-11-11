-- The design system's sidebar on its own with rows made up here, each with
-- a pull request's badge, some with a subagents pill, a status or a time,
-- and titles long enough to truncate: what tests/sidebar_squeeze drives.
-- (`narrow` takes it to 200 points wide, where a row has no room for its
-- title at all.)
ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()

local function pr(n: number, state: string): any
	return { number = n, state = state }
end

local projects: { any } = {
	{
		id = "meridian",
		name = "Meridian",
		hue = "blue",
		sessions = {
			{ id = "m1", title = "Cache invalidation for the image pipeline", status = "done", time = "12h", agent = "openai", device = "Studio", branch = "img/cache", pr = pr(377, "merged") },
			{ id = "m2", title = "Docs sweep", status = "approval", time = "104w", agent = "cursor", device = "Studio", branch = "docs", pr = pr(1042, "open") },
			{ id = "m3", title = "Tidy the tests", time = "1h", agent = "claude", device = "Studio", branch = "tests", pr = pr(9, "merged") },
			{ id = "m4", title = "Migrate the build to the new runner and keep the cache warm", status = "working", time = "now", agent = "claude", device = "Studio", branch = "ci/runner", subagents = 100, pr = pr(51204, "open") },
		},
	},
}

local width = ui.state("squeeze_w", 260)
ui.command {
	id = "narrow",
	title = "The sidebar 200 points wide",
	run = function()
		width:set(200)
	end,
}

return ui.row {
	w_full = true,
	h_full = true,
	ds.Sidebar {
		compact = ui.state("squeeze_compact", true),
		organize = ui.state("squeeze_organize", "project"),
		sort = ui.state("squeeze_sort", "updated"),
		places = ui.state("squeeze_places", { pinned = {}, sections = {} }),
		details = {
			branch = ui.state("d_branch", true),
			pr = ui.state("d_pr", true),
			harness = ui.state("d_harness", true),
			project_icon = ui.state("d_project_icon", true),
			location = ui.state("d_location", true),
		},
		here = "Studio",
		collapsed = ui.state("squeeze_collapsed", false),
		island = false,
		width = width,
		selected = ui.state("squeeze_selected", nil :: string?),
		projects = projects,
		top = 38,
		on_new = function() end,
	},
}

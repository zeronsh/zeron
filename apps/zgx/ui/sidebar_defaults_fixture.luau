-- The sidebar's view options as the app opens with them, untouched: what
-- tests/sidebar_defaults.test.luau reads.
local ds = require("@ds")
ds.setup()
local activity = require("@app/screens/shell/activity")
local options = require("@app/screens/shell/sidebar").options
local seen = ui.state("sidebar_defaults", {} :: { [string]: any })
seen:set({
	activity = activity.on:peek(),
	organize = table.concat(options.organize:peek() or {}, ","),
	sort = options.sort:peek(),
	empty_projects = options.details.empty_projects:peek(),
	compact = options.compact:peek(),
	archived = options.details.archived:peek(),
	harness = options.details.harness:peek(),
	project_icon = options.details.project_icon:peek(),
})
return ui.div { id = "sidebar-defaults" }

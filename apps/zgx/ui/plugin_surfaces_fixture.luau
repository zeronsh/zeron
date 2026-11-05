-- Plugins' pages and tabs in the main window, as screens/plugins hands
-- them on: the shell (A's ui/shell_fixture.luau, as it is) with commands
-- standing in for a plugin listing a page ("pages.add") and opening a tab
-- in the open session's panel ("panel.tabs"). What tests/plugin_surfaces
-- drives.

local node = require("./shell_fixture")
local route = require("@app/screens/shell/route")
local Panel = require("@app/screens/panel")

ui.command {
	id = "page",
	title = "A plugin lists a page",
	run = function()
		ui.shared("plugins.pages", {} :: { any }):set({
			{ plugin = "prs", plugin_name = "PRs", id = "open", title = "Pull requests", icon = "pull-request" },
		})
	end,
}
ui.command {
	id = "tab",
	title = "A plugin opens a tab in the open session's panel",
	run = function()
		local session = route.current:peek().session
		Panel.open_plugin_tab(session, { plugin = "prs", kind = "pr", title = "PR 12", icon = "pull-request", data = { n = 12 } })
	end,
}

return node

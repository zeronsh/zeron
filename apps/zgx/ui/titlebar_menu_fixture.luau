-- The shell with the app's menus as the host serves them where the system
-- has none (src/plugins/host_menu.rs `serve`): the title bar's ☰, and its
-- Alt, for tests/title_bar_menu.test.luau. "fixture.alt" stands in for
-- the host hearing Alt pressed and released alone; "fixture.no_menus" for
-- macOS, whose menu bar has them.

ui.shared("app.menus", nil :: any):set({
	{
		label = "File",
		entries = {
			{ label = "New chat", id = "new-chat", keys = "cmd+n" },
			{ separator = true },
			{ label = "Close", id = "close", keys = "cmd+w" },
		},
	},
	{
		label = "View",
		entries = {
			{ label = "Toggle sidebar", id = "toggle-sidebar", keys = "cmd+b" },
			{ label = "Use Zeron's Template", id = "zeron.use-zeron-template" },
		},
	},
	{ label = "Go", entries = { { label = "Search", id = "search", keys = "cmd+k" } } },
})
local alt = ui.shared("app.menu_alt", false)
ui.command {
	id = "fixture.alt",
	title = "Fixture: Alt pressed and released alone",
	menu = "Fixture",
	run = function()
		alt:set(not alt:peek())
	end,
}
ui.command {
	id = "fixture.no_menus",
	title = "Fixture: no menus served (macOS)",
	menu = "Fixture",
	run = function()
		ui.shared("app.menus", nil :: any):set(nil)
	end,
}

return require("./shell_fixture")

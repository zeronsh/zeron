-- The shell with the window's own controls, as the host serves them where
-- the platform draws none (src/window_controls.rs): for
-- tests/title_bar_controls.test.luau. "fixture.tiling" and
-- "fixture.maximized" flip what the host would say; what the controls ask
-- the host is kept in the state "invoked".

local tiling = ui.state("fixture_tiling", false)
local chrome = ui.shared("window.chrome", nil :: any)
local function tell()
	chrome:set({ tiling = tiling:peek() })
end
tell()
local maximized = ui.shared("window.maximized", false)
local invoked = ui.state("invoked", "")
require("@ds").TitleBar.host.call = function(name: string)
	invoked:set(name)
end
ui.command {
	id = "fixture.tiling",
	title = "Fixture: a tiling compositor, or not",
	menu = "Fixture",
	run = function()
		tiling:set(not tiling:peek())
		tell()
	end,
}
ui.command {
	id = "fixture.maximized",
	title = "Fixture: the window maximized, or not",
	menu = "Fixture",
	run = function()
		maximized:set(not maximized:peek())
	end,
}
ui.command {
	id = "fixture.controls_show",
	title = "Fixture: window controls shown",
	menu = "Fixture",
	run = function()
		require("@ds").TitleBar.window_controls:set("show")
	end,
}
ui.command {
	id = "fixture.controls_hide",
	title = "Fixture: window controls hidden",
	menu = "Fixture",
	run = function()
		require("@ds").TitleBar.window_controls:set("hide")
	end,
}

return require("./titlebar_menu_fixture")

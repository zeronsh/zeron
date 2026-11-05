-- The shell's screen boundary on its own, for tests/shell_in_place.test.luau:
-- a module that isn't there and a screen that throws as it's built each
-- show what failed in their place, and what stands beside them goes on.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local boundary = require("@app/screens/shell/boundary")

-- What the plugin system does for a template that fails: Zeron's own in
-- its place at once (here, a stand-in), and nothing for anything else.
boundary.on_fail(function(module: string, _why: string): Node?
	if module == "@app/screens/shell/templated" then
		return ui.text { id = "built-in", "The built-in screen" }
	end
	return nil
end)

-- What the plugin system does once it has swapped a module for Zeron's
-- own: that screen made again, alone.
ui.command {
	id = "rebuild",
	title = "Make the templated screen again",
	run = function()
		boundary.rebuild("@app/screens/shell/templated")
	end,
}

return ui.column {
	w_full = true,
	h_full = true,
	ui.column { id = "missing", h = 200, boundary.screen("Missing", "@app/screens/shell/nowhere", function(m): Node
		return m.View()
	end) },
	ui.column { id = "throws", h = 200, boundary.screen("Throws", "@app/screens/shell/boundary", function(): Node
		error("a template that throws")
	end) },
	ui.column { id = "templated", h = 100, boundary.screen("Templated", "@app/screens/shell/templated", function(m): Node
		return m.View()
	end) },
	ui.text { id = "beside", "Still here" },
}

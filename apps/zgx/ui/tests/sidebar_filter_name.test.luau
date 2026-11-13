-- A plugin's sidebar filter is named by the host (`sidebar.filter_by`: the
-- name of the plugin allowed to set it, from src/plugins), never by what the
-- plugin writes in its `by`: one can't say it's Zeron or another plugin. On
-- ui/shell_fixture.luau.

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	return app
end

test("the notice names the plugin the host says, not the one it claims", function()
	local app = open()
	app:set_state("sidebar.filter_by", "Focus")
	app:set_state("sidebar.filter", { statuses = { "working" }, by = "Zeron" })
	app:frames(40)
	expect(app:text("#sidebar-filter")).to_contain("Focus is showing working sessions only")
	expect(app:text("#sidebar-filter")).never.to_contain("Zeron")
end)

test("with no name from the host, it's “A plugin”", function()
	local app = open()
	app:set_state("sidebar.filter", { statuses = { "working" }, by = "Zeron" })
	app:frames(40)
	expect(app:text("#sidebar-filter")).to_contain("A plugin is showing working sessions only")
end)

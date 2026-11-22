-- A hook gated on a built-in plugin, for tests/plugins_on.test.luau:
-- shown while `plugins.on("orchestrators")`, naming whether the plugin's
-- app-side module is there. `plugins_on_seed` is the window's word on which
-- plugins run (nothing, by default, as on a fresh install).
local ds = require("@ds")
ds.setup()
local plugins = require("@account/plugins")
local seed = ui.state("plugins_on_seed", nil :: { [string]: boolean }?)
ui.effect(function()
	local now = seed:get()
	if now then
		plugins.set(now)
	end
end)
-- What the app has in the plugin's place with it off (ui/builtin): its
-- parts while their modules are still in ui/, else `false`.
local standin = require("@app/builtin/orchestrators")
return ui.column {
	id = "root",
	ui.text { id = "standin", if type(standin) == "table" then "parts" else "false" },
	ui.show(function(): boolean
		return plugins.on("orchestrators")
	end, ui.text {
		id = "gated",
		function(): string
			return if plugins.module("orchestrators") then "module" else "no module"
		end,
	}),
}

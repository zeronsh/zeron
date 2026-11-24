-- The account's deploy tokens, mounted on their own for an author of plugins
-- and templates (the page gives both): what tests/deploy_tokens.test.luau
-- drives, with the store in ui/store_stub.luau.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local keys = require("@app/screens/plugins/keys")
require("./store_stub")

local templates = ui.state("templates", true)
ui.command { id = "no_templates", title = "No templates", run = function()
	templates:set(false)
end }
ui.command { id = "zh", title = "zh", run = function()
	ui.locale:set("zh-Hans")
end }

return ui.scroll {
	id = "fixture-scroll",
	w_full = true,
	h_full = true,
	keys.Section {
		plugins = function()
			return { { id = "focus-timer", name = "Focus timer" }, { id = "tidy", name = "Tidy" } }
		end,
		templates = function()
			return if templates:get() then { { id = "midnight", name = "Midnight" } } else {}
		end,
	},
}

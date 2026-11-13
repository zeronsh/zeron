--!strict
-- Templates in the main window: their page in Settings, beside Plugins.
-- Required once, as the main window starts (app.luau, through
-- screens/plugins). Never a template's own (src/plugins/live.rs): this page
-- is how you get back to Zeron's.

local settings = require("@app/screens/shell/settings")
local page = require("@app/screens/templates/page")

settings.add {
	id = "templates",
	title = "Templates",
	icon = "gallery",
	view = page.View,
	group = "Customize",
	before = "archived",
}

return {}

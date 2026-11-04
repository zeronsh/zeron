--!strict
-- Themes in the main window: their page in Settings, beside Plugins and
-- Templates. Required once, as the main window starts (app.luau, through
-- screens/plugins).

local settings = require("@app/screens/shell/settings")
local page = require("@app/screens/themes/page")

settings.add {
	id = "themes",
	title = "Themes",
	icon = "palette",
	view = page.View,
	group = "Customize",
	before = "archived",
}

return {}

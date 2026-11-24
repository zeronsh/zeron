--!strict
-- Plugins, as the store's screens take them (screens/plugins/store_kind).

local kinds = require("@app/screens/plugins/store_kind")

local kind: kinds.Kind = {
	id = "plugin",
	keys = "store",
	store = require("@app/screens/plugins/store"),
	nav = require("@app/screens/plugins/nav"),
	state = require("@app/screens/plugins/state"),
	settings = require("@app/screens/plugins/settings"),
}

return kind

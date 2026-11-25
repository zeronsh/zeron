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
	categories = true,
	checks = {
		{ id = "readme", rule = "missing_readme" },
		{ id = "description", rule = "listing", field = "description" },
		{ id = "icon", rule = "listing", field = "icon" },
		{ id = "categories", rule = "listing", field = "categories" },
		{ id = "version", rule = "not_semver" },
	},
	fields = { icon = true, categories = true, description = true, name = true, keywords = true, screenshots = true, homepage = true, repository = true, license = true },
	rules = { missing_readme = true, not_semver = true, id_reserved = true, too_large = true, binary_file = true, no_notes = true, raw_html = true, footnote = true, math = true, not_newer = true, version_exists = true },
}

return kind

--!strict
-- Templates, as the store's screens take them (screens/plugins/store_kind):
-- this device's, each by the store's record of it; pictures in a grid; a
-- template asks for nothing, so what it is is said once instead.

local kinds = require("@app/screens/plugins/store_kind")
local state = require("@app/screens/templates/state")

-- What's installed, as the store's screens read a thing installed.
local shown = ui.computed(function(): { any }
	local out: { any } = {}
	for _, t in state.list:get() do
		local source: any = t.imported and t.imported.source
		table.insert(out, { id = t.id, version = t.version, status = t.status, source = source, update = t.update, blocked = t.blocked })
	end
	return out
end)

local words: { [string]: string } = {}
for _, key in {
	"search",
	"none_verified",
	"offline_title",
	"offline_body",
	"failed_title",
	"failed_body",
	"more_failed",
	"nothing_here",
	"empty_title",
	"empty_body",
	"count",
	"list_title",
} do
	words["plugins.store." .. key] = "templates.store." .. key
end

local kind: kinds.Kind = {
	id = "template",
	keys = "template_store",
	store = require("@app/screens/templates/store"),
	nav = require("@app/screens/templates/nav"),
	state = { shown = shown },
	words = words,
	cards = true,
	fact = "templates.store.runs_as_zeron_list",
}

return kind

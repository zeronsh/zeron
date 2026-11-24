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
		table.insert(out, {
			id = t.id,
			version = t.version,
			status = t.status,
			active = t.active,
			source = source,
			update = t.update,
			blocked = t.blocked,
			edited = (t :: any).edited,
		})
	end
	return out
end)

local words: { [string]: string } = {
	["plugins.page.couldnt_install"] = "templates.page.couldnt_install",
	["plugins.page.couldnt_update"] = "templates.page.couldnt_update",
	["plugins.page.couldnt_switch"] = "templates.page.couldnt_use",
}
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
	"not_found_title",
	"not_found_body",
	"browse_all",
	"report.link",
	"report.title",
	"report.body",
	"other_title",
	"other_body",
	"updates_body",
	"update_body",
	"update_asks_first",
	"blocked_body",
	"blocked_body_reason",
	"couldnt_updates",
} do
	words["plugins.store." .. key] = "templates.store." .. key
end

local kind: kinds.Kind = {
	id = "template",
	keys = "template_store",
	store = require("@app/screens/templates/store"),
	nav = require("@app/screens/templates/nav"),
	state = {
		shown = shown,
		-- Which updates each takes here: this device's own choice for none.
		store_updates = ui.shared("templates.store_updates", nil :: any),
		-- Whether a package of it waits for you.
		awaits = function(id: string): boolean
			for _, p in state.pending:get() do
				local from: any = (p :: any).store
				if type(from) == "table" and from.id == id then
					return true
				end
			end
			return false
		end,
		review = function(id: string)
			for _, p in state.pending:peek() do
				local from: any = (p :: any).store
				if type(from) == "table" and from.id == id then
					state.review(p.pending)
				end
			end
		end,
		install = function(from: any): any
			return state.import { store = from.store, version = from.version }
		end,
		update = function(id: string): any
			for _, t in state.list:peek() do
				local source: any = t.imported and t.imported.source
				if type(source) == "table" and source.kind == "store" and source.id == id then
					return state.update(t.id)
				end
			end
			return nil
		end,
		use = function(id: string): any
			return state.use(id)
		end,
		check_updates = function()
			local p: any = state.check_updates()
			if p ~= nil then
				p:after(function(_: any) end, function(_: any) end)
			end
		end,
	},
	words = words,
	cards = true,
	fact = "templates.store.runs_as_zeron_list",
}

return kind

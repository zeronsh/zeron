--!strict
-- PLUGIN_NAME's settings page, drawn by the plugin (`zeron.settings_page`, no
-- permission). Zeron draws the page's header (its icon, name and the line
-- under it) and the rows zeron-plugin.json's "settings" declares; this draws
-- what comes after them, in Settings, under Plugins, while it's on.
--
-- Build it as Zeron's pages are (the guide's "Settings pages"): from
-- @ds/settings' Section, Group and Row and Zeron's controls, tokens only,
-- a list of sections. Check it with plugin_status { page = "settings",
-- snapshot = true, open = true }: a picture of the page, and hints of
-- anything off the design system.

local zeron = require("./zeron")("PLUGIN_ID")
local Settings = require("@ds/settings")
local Button = require("@ds/button")
local IconButton = require("@ds/icon_button").IconButton
local EmptyState = require("@ds/empty_state")

-- What it keeps (`zeron.store`: kept for this plugin alone, across runs).
type Item = { id: string, name: string }
local items = zeron.store("items", {} :: { Item })

local function add()
	local list = table.clone(items:peek())
	local n = #list + 1
	table.insert(list, { id = `item-{math.floor(ui.now())}-{n}`, name = `{zeron.s("settings.items.new")} {n}` })
	items:set(list)
end

local function remove(id: string)
	local list = {}
	for _, it in items:peek() do
		if it.id ~= id then
			table.insert(list, it)
		end
	end
	items:set(list)
end

-- An item's row: its name, and the way to remove it at its end.
local function item_row(it: Item): Node
	return Settings.Row {
		label = it.name,
		control = IconButton {
			icon = "trash-bin-minimalistic",
			size = "sm",
			tone = "quiet",
			label = zeron.s("settings.items.remove", { name = it.name }),
			on_click = function()
				remove(it.id)
			end,
		},
	}
end

zeron.settings_page {
	keywords = { zeron.t("settings.items.title") },
	build = function(_page: any): any
		local list = ui.computed(function(): { Item }
			return items:get()
		end)
		return {
			Settings.Section {
				title = zeron.t("settings.items.title") :: any,
				description = zeron.t("settings.items.description") :: any,
				trail = Button { zeron.t("settings.items.add"), tone = "secondary", size = "sm", on_click = add },
				ui.show(function(): boolean
					return #list:get() > 0
				end, Settings.Each(list, item_row, {
					key = function(it: Item): string
						return it.id
					end,
				}), Settings.Group {
					ui.column {
						w_full = true,
						py = "$space.6",
						px = "$space.4",
						EmptyState {
							size = "sm",
							icon = "list",
							title = zeron.t("settings.items.empty.title"),
							body = zeron.t("settings.items.empty.body"),
							action = { label = zeron.t("settings.items.empty.action"), on_click = add },
						},
					},
				}),
			},
		}
	end,
}

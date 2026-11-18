--!strict
-- Snippets: text you use often, a click away in the message box ("ui.add"
-- for its button beside the message box's other buttons, "compose" to put a
-- snippet in the draft; you still send it).
--
-- Its settings are both kinds a plugin has: two rows its manifest declares
-- (Zeron draws them: whether the button shows, where a snippet goes), and a
-- page of its own after them, where the snippets themselves are kept
-- (settings_page.luau, `zeron.settings_page`). That page is the example to
-- copy for a settings page a plugin draws itself.

local zeron = require("./zeron")("settings-page")
local IconButton = require("@ds/icon_button").IconButton
local Menu = require("@ds/menu")
local Glass = require("@ds/glass")
local snippets = require("./snippets")
require("./settings_page")

local show = zeron.setting("show")
local insert = zeron.setting("insert")

zeron.place {
	at = "#composer-toolbar",
	after = function(): Node
		local open = ui.use_state(false)
		local trigger = IconButton {
			id = "snippets-button",
			icon = "document",
			label = "Snippets",
			tone = "quiet",
			lit = open,
			on_click = function()
				open:set(true)
			end,
		}
		return ui.show(function(): boolean
			return show:get() ~= false
		end, ui.popover(Glass.style({
			open = open,
			anchor = trigger,
			label = "Snippets",
			placement = { side = "top", align = "start", offset = 4 },
			w = ui.theme.get("zeron.menu.width") :: number?,
			p = 0,
			gap = 0,
			rounded = "$radius.lg",
			build = function(): Node
				local rows: { Node } = {}
				for _, s in snippets.list() do
					table.insert(rows, Menu.Row {
						label = s.name,
						role = "menuitem",
						on_click = function()
							open:set(false)
							zeron.compose(snippets.fill(s), { replace = insert:peek() == "replace" })
						end,
					})
				end
				if #rows == 0 then
					table.insert(rows, Menu.Row { label = "No snippets yet", disabled = true })
				end
				return Menu.Group(rows :: any)
			end,
		}, "menu")))
	end,
}

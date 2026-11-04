-- The command palette over a root whose items are made fresh on every
-- call (as the app's are: sessions come and go), for
-- tests/command_palette_order.test.luau. Narrowing a search keeps the
-- previous hits, which then are not in the new list.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local CommandPalette = require("@ds/command_palette")

local function items(): { CommandPalette.Item }
	local list: { CommandPalette.Item } = {}
	for i, label in { "Refresh models", "Restart engine", "Reload window", "Open settings" } do
		table.insert(list, { id = `cmd{i}`, label = label, section = "Actions", run = function() end })
	end
	for i, label in { "Rebase the branch", "Retry uploads", "Rename project" } do
		table.insert(list, {
			id = `s{i}`,
			label = label,
			section = "Sessions",
			session = { id = `s{i}`, title = label, state = "idle" } :: any,
			run = function() end,
		})
	end
	return list
end

local open = ui.state("test_palette_open", true)
local root: CommandPalette.Page = { id = "root", title = "Zeron", items = items }

return CommandPalette { open = open, root = root, recent = ui.state("test_palette_recent", {} :: { string }) }

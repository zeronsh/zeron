-- Rebinding a command's shortcut while the app runs: the menu (and so the
-- key that fires) keeps the key the command was declared with.
--
--   ZGX_CHECK=0 zgx run .
--   ⌘J says "ran". Click "Bind to ⌘L": the label says ⌘L, but ⌘L does
--   nothing and ⌘J still runs it. Only a reload of the scripts (save this
--   file) re-declares the command with ⌘L.

local key = ui.persist("rebind_key", "cmd+j")
local ran = ui.state("rebind_ran", 0)

ui.command {
	id = "demo.toggle",
	title = "Toggle the thing",
	menu = "View",
	-- Read once, as the script loads: a string is all `key` takes.
	key = key:peek(),
	run = function()
		ran:add(1)
	end,
}

return ui.column {
	p = 24,
	gap = 12,
	ui.text { function() return `Bound to {key:get()} · ran {ran:get()} times` end },
	ui.button {
		"Bind to ⌘L",
		on_click = function()
			key:set("cmd+l")
		end,
	},
	ui.button {
		"Bind to ⌘J",
		on_click = function()
			key:set("cmd+j")
		end,
	},
}

-- The composer's trays with what a session runs in the background, for
-- tests/running_tray.test.luau: the list (`test_running_tasks`) set whole
-- each time, as the host publishes it, and the tray open
-- (`test_running_tray`). Here, beside app.luau, as a mounted script loads
-- modules from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
local Dock = require("@ds/composer_dock")
ds.setup()

local calls = ui.state("calls", {} :: { string })
local function call(what: string)
	local list = table.clone(calls:peek())
	table.insert(list, what)
	calls:set(list)
end

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.4",
	justify = "end",
	Dock.Dock {
		tasks = ui.state("test_running_tasks", {} :: { Dock.Task }),
		-- (Empty unless a test fills them: the deck's tabs then.)
		queue = ui.state("test_deck_queue", {} :: { Dock.Queued }),
		todos = ui.state("test_deck_todos", {} :: { Dock.Todo }),
		alerts = ui.state("test_deck_alerts", {} :: { Dock.Alert }),
		on_alert_action = function(id: string)
			call(`alert:{id}`)
		end,
		on_dismiss_alert = function(id: string)
			call(`dismiss:{id}`)
		end,
		open = ui.state("test_running_tray", nil :: Dock.Tray?),
		on_stop_task = function(id: string)
			call(`stop:{id}`)
		end,
		on_stop_all = function()
			call("stop_all")
		end,
		on_open_task = function(id: string)
			call(`open:{id}`)
		end,
	},
}

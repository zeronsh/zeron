-- The shell's fixture with the app's own toasts on command, for the host's
-- toasts beside them (src/plugins/host_bar.rs): "fixture.toast" shows one
-- of ds/toast's (another each time), "fixture.toasts_clear" puts them away.
-- With `ZERON_ZGX_HOST_TOASTS=1` (debug builds), the host's toasts show
-- over it too.

local node = require("./shell_fixture")
local toast = require("@ds/toast")

local shown: { number } = {}
local count = ui.state("host_toasts_fixture_count", 0)
ui.command {
	id = "fixture.toast",
	title = "Fixture: one of the app's toasts",
	menu = "Fixture",
	run = function()
		count:set(count:peek() + 1)
		table.insert(shown, toast.show { title = `Pushed to origin ({count:peek()})`, body = "main is up to date.", tone = "success", duration = 0 })
	end,
}
ui.command {
	id = "fixture.toasts_clear",
	title = "Fixture: put the app's toasts away",
	menu = "Fixture",
	run = function()
		for _, id in shown do
			toast.dismiss(id)
		end
		shown = {}
	end,
}

return node

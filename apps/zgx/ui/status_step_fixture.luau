-- A working ring and a pulsing dot, shown or not (`status_step_shown`), for
-- tests/status_step.test.luau: off macOS they step on one shared clock that
-- runs only while one shows.
local ds = require("@ds")
ds.setup()
local status = require("@ds/status")
local shown = ui.state("status_step_shown", true)
return ui.column {
	ui.show(function(): boolean
		return shown:get()
	end, ui.row {
		id = "status-step-row",
		status.StatusIcon { kind = "working", size = 16 },
		status.StatusDot { kind = "working" },
		status.Spinner {},
	}),
}

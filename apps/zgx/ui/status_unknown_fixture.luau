-- A status icon following a state whose kind can be one ds/status doesn't
-- know (nil, as a row being torn down reads it, or a kind a newer engine
-- sends), for tests/status_unknown.test.luau.
local ds = require("@ds")
ds.setup()
local status = require("@ds/status")
local kind = ui.state("status_unknown_kind", "working" :: any)
return ui.column {
	ui.row {
		id = "status-unknown-row",
		status.StatusIcon {
			kind = function(): any
				return kind:get()
			end,
			size = 16,
		},
	},
}

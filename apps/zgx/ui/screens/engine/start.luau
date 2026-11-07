-- The engine can't start (@account/engine): said in the page's place, with
-- Retry. Before a minute has passed nothing is said; the page loads.

local ds = require("@ds")
local engine = require("@account/engine")

return function(): Node
	return ui.column {
		id = "engine-start-failed",
		w_full = true,
		h_full = true,
		min_w = 0,
		items = "center",
		justify = "center",
		p = "$space.4",
		ds.EmptyState {
			icon = "danger-triangle",
			title = ui.t("engine.start.failed_title") :: any,
			body = ui.t("engine.start.failed_body") :: any,
			action = {
				label = ui.t("common.retry") :: any,
				on_click = function()
					-- (Each time through `engine`, so a stub stands in for it.)
					engine.retry()
				end,
			},
			fill = true,
		},
	}
end

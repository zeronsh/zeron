-- The orchestration live pieces alone, for tests/orch_live.test.luau: a
-- status held by ds/orch_hold (the state `kind`, the held value shown as
-- `#held`), segmented cells (`#cells` in a row, `#cells-fill` stretched;
-- from `done` of 8) and a sparkline.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local status = require("@ds/status")
local Cells = require("@ds/orch_cells")
local Spark = require("@ds/orch_spark")
local hold = require("@ds/orch_hold")

local kind = ui.state("kind", "working")
local done = ui.state("done", 3)

local Held = ui.component(function(): Node
	local held = hold.value(function(): string
		return kind:get()
	end)
	return ui.row {
		gap = "$space.2",
		items = "center",
		status.StatusIcon { kind = held :: any },
		ui.text { held, id = "held" },
	}
end)

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.4",
	gap = "$space.4",
	bg = "$background",
	Held {},
	-- (In a row with words before it, as a card's or a thread's are.)
	ui.row {
		gap = "$space.2",
		items = "center",
		ui.text { "Steps" },
		Cells { id = "cells", done = function(): number
			return done:get()
		end, total = 8 },
		ui.text { "3/8" },
	},
	Cells { id = "cells-fill", done = function(): number
		return done:get()
	end, total = 8, fill = true },
	Spark.Spark {
		id = "spark",
		samples = function(): { Spark.Sample }
			local out = {}
			for i = 1, 30 do
				table.insert(out, { at = 1000 + i * 5, v = (math.sin(i) + 1) / 2 })
			end
			return out
		end,
		events = function(): { Spark.Event }
			return { { at = 1100, tone = "danger" } }
		end,
		label = "CPU",
		value = "50%",
	},
}

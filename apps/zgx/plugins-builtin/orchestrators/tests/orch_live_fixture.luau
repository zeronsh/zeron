-- The orchestration live pieces alone, for tests/orch_live.test.luau: a
-- status held by ds/orch_hold (the state `kind`, the held value shown as
-- `#held`), segmented cells (`#cells` in a row, `#cells-fill` stretched;
-- from `done` of 8), a label and its rolling figure (`#cpu-label`,
-- `#counter`, from `cpu`) and a sparkline.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local status = require("@ds/status")
local Cells = require("@orchestrators/ds/orch_cells")
local Spark = require("@orchestrators/ds/orch_spark")
local hold = require("@orchestrators/ds/orch_hold")
local Counter = require("@orchestrators/ds/orch_counter")

local kind = ui.state("kind", "working")
local done = ui.state("done", 3)
local cpu = ui.state("cpu", 4)

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

-- Where ds/orch_spark places a short line and a full one (300 wide, a
-- six-minute window), for the test to read: `spark_place`.
local function line(n: number): { Spark.Sample }
	local out = {}
	for i = 1, n do
		table.insert(out, { at = 1000 + (i - 1) * 5, v = 0.5 })
	end
	return out
end
local short, long = line(5), line(100)
local xs, short_full = Spark.place(short, 360, 300, short[#short].at)
local xl, long_full = Spark.place(long, 360, 300, long[#long].at)
local _, none_full = Spark.place({}, 360, 300, nil)
ui.state("spark_place", {
	short_first = xs(short[1].at),
	short_head = xs(short[#short].at),
	short_full = short_full,
	long_first = xl(long[1].at),
	long_head = xl(long[#long].at),
	long_full = long_full,
	none_full = none_full,
})

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.4",
	gap = "$space.4",
	bg = "$background",
	Held {},
	-- A label and its rolling figure, as a machine's head has them.
	ui.row {
		gap = "$space.1",
		items = "center",
		ui.text { "CPU", id = "cpu-label", text_size = "$font.size.xs" },
		Counter { id = "counter", value = function(): number
			return cpu:get()
		end, suffix = "%", anchor = "end" },
	},
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

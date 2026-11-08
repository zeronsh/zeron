-- An agent's presence, as lists draw it: `presence_rows` rows (12 by
-- default), each a working agent's sweep before its title, or, with
-- `presence_glyph` "ring", the ring loading draws; above them a sub-agent,
-- one connecting and one waiting. What idle CPU and frames are measured
-- with (`fixture=presence_fixture.luau`), and the presence test's ground.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local status = require("@ds/status")

local rows = ui.shared("presence_rows", 12)
local glyph = ui.shared("presence_glyph", "sweep")

local function row(i: number): Node
	return ui.row {
		w = 320,
		h = 28,
		px = "$space.2",
		gap = "$space.2",
		items = "center",
		ui.switch(glyph, {
			sweep = status.StatusIcon { kind = "working", size = 14 },
			ring = status.Spinner { size = 14 },
		}),
		ui.text { `Agent {i} at work`, text_size = "$font.size.md", color = "$text.primary" },
	}
end

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.6",
	gap = "$space.1",
	bg = "$background",
	ui.row {
		id = "kinds",
		gap = "$space.4",
		ds.Status { kind = "working", who = "subagent", label = "Sub-agent" },
		ds.Status { kind = "connecting" },
		ds.Status { kind = "waiting" },
	},
	ui.column {
		id = "rows",
		ui.each(
			ui.computed(function(): { number }
				local out = {}
				for i = 1, rows:get() do
					out[i] = i
				end
				return out
			end),
			row
		),
	},
}

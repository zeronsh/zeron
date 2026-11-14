-- An agent's presence, as a list and a chat draw it: `presence_rows` rows
-- (12 by default), each a working agent's quiet mark before its title (or,
-- with `presence_glyph` "ring", the ring loading draws); with
-- `presence_primary`, a chat's foot sweeping and `presence_subagents`
-- running sub-agents (3), of which two sweep. The `presence_count` command
-- says how many glyphs move (`presence_moving`). What idle CPU and frames
-- are measured with (`fixture=presence_fixture.luau`), and the presence
-- test's ground.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local status = require("@ds/status")

local rows = ui.shared("presence_rows", 12)
local glyph = ui.shared("presence_glyph", "sweep")
local primary = ui.shared("presence_primary", true)
local subagents = ui.shared("presence_subagents", 3)
local moving = ui.shared("presence_moving", 0)
ui.command {
	id = "presence_count",
	title = "How many glyphs move",
	run = function()
		moving:set(status.moving())
	end,
}

local function count(n: () -> number): any
	return ui.computed(function(): { number }
		local out = {}
		for i = 1, n() do
			out[i] = i
		end
		return out
	end)
end

local function line(icon: Node, text: string): Node
	return ui.row {
		w = 320,
		h = 28,
		px = "$space.2",
		gap = "$space.2",
		items = "center",
		icon,
		ui.text { text, text_size = "$font.size.md", color = "$text.primary" },
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
		ds.Status { kind = "connecting" },
		ds.Status { kind = "waiting" },
		ds.StatusDot { kind = "working", size = 8 },
	},
	ui.column {
		id = "rows",
		ui.each(count(function()
			return rows:get()
		end), function(i: number): Node
			return line(
				ui.switch(glyph, {
					sweep = status.StatusIcon { kind = "working", size = 14 },
					ring = status.Spinner { size = 14 },
				}),
				`Agent {i} at work`
			)
		end),
	},
	ui.show(
		primary,
		ui.column {
			id = "chat",
			line(status.StatusIcon { kind = "working", size = 16, role = "primary" }, "Noodling…"),
			ui.each(count(function()
				return subagents:get()
			end), function(i: number): Node
				return line(status.StatusIcon { kind = "working", size = 14, who = "subagent", role = "primary" }, `Sub-agent {i}`)
			end),
		}
	),
}

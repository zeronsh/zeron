-- An open group of tool calls alone, for what a row costs to build
-- (src/activity_cost_tests.rs `activity_row_cost`): `bench.rows` calls,
-- closed, of the kinds a reply's are, in a window tall enough to show them.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Activity = require("@ds/activity")

local count = ui.shared("bench.rows", 0)
-- (Shown once its calls are there: they're made as rows coming into view are,
-- not as calls arriving.)
local shown = ui.shared("bench.shown", false)
local KINDS = {
	{ kind = "exec", verb = "Ran", target = "cargo test --package part -- --nocapture" },
	{ kind = "read", verb = "Read", target = "chat.rs" },
	{ kind = "search", verb = "Searched", target = "fn part" },
	{ kind = "edit", verb = "Edited", target = "transcript.luau", added = 3, removed = 1 },
}
local made: { any } = {}
local tools = ui.computed(function(): { any }
	local n = count:get()
	for i = #made + 1, n do
		local k = KINDS[(i - 1) % #KINDS + 1]
		made[i] = { id = `t{i}`, kind = k.kind, verb = k.verb, target = `{k.target} {i}`, added = k.added, removed = k.removed }
	end
	return table.move(made, 1, n, 1, {})
end)
return ui.scroll {
	w_full = true,
	h_full = true,
	ui.column {
		w_full = true,
		p = "$space.6",
		ui.show(function(): boolean
			return shown:get() == true
		end, Activity {
			key = "bench",
			tools = function()
				return tools:get()
			end,
			live = function()
				return true
			end,
			summary = function()
				return "Ran commands"
			end,
		}),
	},
}

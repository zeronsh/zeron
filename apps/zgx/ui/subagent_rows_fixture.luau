-- A reply's work group with subagents in it, one still at work.
ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Activity = require("@ds/activity")

local status = ui.shared("subagent_status", "running")
local opened = ui.shared("subagent_opened", "")
return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.6",
	Activity {
		tools = function()
			return {
				{ id = "s1", kind = "search", verb = "Searched", target = "the docs" },
				{
					id = "a1",
					kind = "agent",
					verb = "Ran a subagent",
					target = "Read the articles",
					subagent = { ref = "doc--sub--1", status = status:get(), title = "Read the articles" },
				},
				{
					id = "a2",
					kind = "agent",
					verb = "Ran a subagent",
					target = "Read the feeds",
					subagent = { ref = "doc--sub--2", status = "done", title = "Read the feeds" },
				},
			}
		end,
		open_subagent = function(sub: any)
			opened:set(sub.ref)
		end,
	},
}

-- The gallery's Subagents page: a reply's subagent block (ds/subagents), its
-- subagents from three harnesses running, then finishing one by one; the
-- block folds itself once all are done, unless you chose.

local ds = require("@ds")
local Subagents = require("@ds/subagents")

local NOW = os.time()
local function start(): { any }
	return {
		{ id = "a", title = "Map the parser", status = "running", mark = "claude", model = "Haiku 4.5", step = "Explore", started = NOW - 52, stoppable = true, openable = true },
		{ id = "b", title = "Review the API", status = "running", mark = "openai", model = "GPT-5.5", step = "Reading routes.ts", started = NOW - 31, stoppable = true, openable = true },
		{ id = "c", title = "Check the lockfile", status = "running", mark = "grok", model = "Grok 4.7", step = "cargo tree -d", started = NOW - 12, stoppable = true, openable = true },
	}
end
local RESULTS = { a = "Found 3 callers of parse().", b = "The API looks fine.", c = "One duplicate: syn 1 and 2." }
local items = ui.state("subagents_demo_items", start())
local said = ui.state("subagents_demo_said", "Stop or open a subagent.")

return {
	id = "subagents",
	title = "Subagents",
	about = "A reply's subagents, at its top: their marks stacked, how many work, how long they've taken; open while any runs, folded once all are done. A row stops one while it runs and opens its lane once it's done.",
	demo = function(): Node
		return ui.column {
			w_full = true,
			max_w = 640,
			gap = "$space.5",
			ui.row {
				gap = "$space.2",
				flex_wrap = true,
				ds.Button { tone = "secondary", size = "sm", on_click = function()
					local now = table.clone(items:peek())
					for i, s in now do
						if s.status == "running" then
							local done = table.clone(s)
							done.status = "done"
							done.result = RESULTS[s.id]
							done.ended = os.time()
							now[i] = done
							break
						end
					end
					items:set(now)
				end, "Finish a subagent" },
				ds.Button { tone = "secondary", size = "sm", on_click = function()
					items:set(start())
				end, "Start over" },
			},
			Subagents {
				key = "demo",
				items = function(): { any }
					return items:get()
				end,
				on_stop = function(s: any, settle: (boolean) -> ())
					said:set(`Stopped “{s.title}”`)
					settle(false)
				end,
				on_open = function(s: any)
					said:set(`Opened “{s.title}”`)
				end,
			},
			ds.Text { function(): string
				return said:get()
			end, style = "caption", tone = "secondary" },
		}
	end,
}

-- The gallery's Session lanes page: a session's side chats and subagents as
-- its details card lists them (ds/session_lanes), with lanes that come, run
-- and finish, and the card with nothing in it yet.

local ds = require("@ds")
local SessionLanes = require("@ds/session_lanes")

local NOW = os.time()
local side = ui.state("lanes_demo_side", {
	{ id = "s1", title = "Why does the sync test flake?", status = "working", agent = "claude", updated = NOW - 40 },
	{ id = "s2", title = "Draft the release notes", agent = "openai", updated = NOW - 1800 },
	{ id = "s3", title = "New side chat", updated = NOW - 7200 },
} :: { any })
local subs = ui.state("lanes_demo_subs", {
	{ ref = "r1", title = "scan the repo", status = "running", agent = "claude", at = NOW - 90 },
	{ ref = "r2", title = "read the docs", status = "running", agent = "claude", at = NOW - 60 },
	{ ref = "r3", title = "find the flaky test", status = "done", agent = "claude", at = NOW - 900 },
	{ ref = "r4", title = "check the lockfile", status = "failed", agent = "claude", at = NOW - 1200 },
} :: { any })
local said = ui.state("lanes_demo_said", "Pick a lane.")

local function card(list: () -> { any }, subagents: () -> { any }): Node
	return SessionLanes {
		session = function(): string?
			return "demo"
		end,
		side = list,
		subagents = subagents,
		on_new = function()
			said:set("New side chat")
		end,
		on_fork = function()
			said:set("Fork chat")
		end,
		on_open_side = function(c: any)
			said:set(`Opened side chat “{c.title}”`)
		end,
		on_open_subagent = function(s: any)
			said:set(`Opened subagent “{s.title}”`)
		end,
	}
end

return {
	id = "session-lanes",
	title = "Session lanes",
	about = "A session's side chats and subagents, as its details card lists them: side chats newest first, subagents running first with the finished folded, each opening a tab of its own.",
	demo = function(): Node
		return ui.column {
			w_full = true,
			gap = "$space.5",
			ui.row {
				gap = "$space.2",
				flex_wrap = true,
				ds.Button { tone = "secondary", size = "sm", on_click = function()
					local now = table.clone(side:peek())
					table.insert(now, 1, { id = `s{#now + 1}`, title = "A new side chat", status = "working", agent = "claude", updated = os.time() })
					side:set(now)
				end, "Add a side chat" },
				ds.Button { tone = "secondary", size = "sm", on_click = function()
					local now = table.clone(subs:peek())
					for i, s in now do
						if s.status == "running" then
							local done = table.clone(s)
							done.status = "done"
							now[i] = done
							break
						end
					end
					subs:set(now)
				end, "Finish a subagent" },
			},
			ui.row {
				gap = "$space.6",
				items = "start",
				flex_wrap = true,
				card(function(): { any }
					return side:get()
				end, function(): { any }
					return subs:get()
				end),
				card(function(): { any }
					return {}
				end, function(): { any }
					return {}
				end),
			},
			ds.Text { function(): string
				return said:get()
			end, style = "caption", tone = "secondary" },
		}
	end,
}

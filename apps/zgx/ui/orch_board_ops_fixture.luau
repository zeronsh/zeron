-- The board's account calls (account/orchestration) with the host's op
-- standing in: each `Orchestrate` op they send is kept in `sent`, in order,
-- and answered `{ id }` (a new card's id is `it-new`). A command runs each
-- group of calls (ui/tests/orch_board_ops.test.luau).

local orchestration = require("@account/orchestration")

local sent = ui.state("sent", {} :: { any })
orchestration.op = function(op: any): any
	sent:push(op)
	return {
		after = function(self: any, done: ((any) -> ())?, _failed: any): any
			if done then
				done({ id = if op.op == "upsertItem" then "it-new" else op.id })
			end
			return self
		end,
	}
end

local function run(id: string, f: () -> ())
	ui.command { id = id, title = id, run = function()
		sent:set({})
		f()
	end }
end

run("actions", function()
	orchestration.close_item("it-1", false)
	orchestration.close_item("it-1", true)
	orchestration.reopen_item("it-1")
	orchestration.hold_item("it-1", true)
	orchestration.hold_item("it-1", false)
	orchestration.seen_item("it-1")
	orchestration.pause_item("it-1")
	orchestration.resume_item("it-1", "active")
	orchestration.start_item("it-1")
	orchestration.remove_item("it-1")
	orchestration.request_changes("it-1", "tighten it")
end)

local base = { orchestrator_id = "orc-1", title = "T", detail = "D", priority = "normal", threads = { "th-1" }, links = {}, order = 3 }
for _, state in { "active", "done", "dropped", "inbox", "paused" } do
	run(`restore_{state}`, function()
		local item = table.clone(base)
		item.id = "it-1"
		item.state = if state == "paused" then "blocked" else state
		item.paused_at = if state == "paused" then 1700000000 else nil
		orchestration.restore_item(item)
	end)
end

run("new_task", function()
	orchestration.new_task("Speed up the sidebar", { id = "orc-ci", name = "CI speed" }, "bd-chat")
	orchestration.new_task("Look at the logs", nil, "bd-chat")
end)

return ui.div { id = "ops-fixture" }

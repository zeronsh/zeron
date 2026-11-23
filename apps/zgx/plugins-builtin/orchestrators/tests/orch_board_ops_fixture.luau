-- The board's account calls (account/orchestration) with the host's op
-- standing in: each `Orchestrate` op they send is kept in `sent`, in order,
-- and answered `{ id }` (a new card's id is `it-new`). A command runs each
-- group of calls (ui/tests/orch_board_ops.test.luau).

local orchestration = require("@orchestrators/account/orchestration")

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

-- A delete waits for its Undo (here, an app without the host's wait: the
-- scripts wait, `wait_ms` short): undone, nothing is sent.
run("remove_later", function()
	orchestration.remove_later("it-1", 50)
	orchestration.remove_later("it-2", 50)
	orchestration.undo_remove("it-2")
end)

run("new_task", function()
	orchestration.new_task("Speed up the sidebar", { id = "orc-ci", name = "CI speed" }, "bd-chat")
	orchestration.new_task("Look at the logs", nil, "bd-chat")
end)

return ui.div { id = "ops-fixture" }

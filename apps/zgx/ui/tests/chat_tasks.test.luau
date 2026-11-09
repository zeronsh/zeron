-- A chat's background processes, as Zeron's tray has them
-- (background_tray.rs): Stop asks the chat's host to stop one, and says so
-- if it couldn't.

test("Stop on a background process asks its host, and says when it couldn't", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:command("task_running")
	app:frames(30)
	-- The tray stays folded to its header until opened.
	expect(app:has('role:button "Open Running"')).to_be(true)
	expect(app:has('role:tab "Fold Running"')).to_be(false)
	app:click('role:button "Open Running"')
	app:frames(30)
	app:click('role:button "Stop npm run dev"')
	app:frames(10)
	expect(app:state("calls")).to_contain("stop_task:s2:bg1")
	expect(app:has('~"Couldn\'t stop that process"')).to_be(false)
	app:command("stop_fails")
	app:click('role:button "Stop npm run dev"')
	app:frames(10)
	-- Said as an alert of the composer's deck, a tab beside Running.
	expect(app:has('role:tab "Open Alerts"')).to_be(true)
	app:click('role:tab "Open Alerts"')
	app:frames(30)
	expect(app:has('~"Couldn\'t stop that process"')).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("a background process outlives the turn: its tray stays as the turn ends", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:command("start_reply")
	app:command("task_running")
	app:frames(30)
	expect(app:has('role:button "Open Running"')).to_be(true)
	app:command("end_reply")
	-- Through the turn's end, every frame: the tray is there.
	for _ = 1, 30 do
		app:frames(1)
		expect(app:has('role:button "Open Running"')).to_be(true)
	end
	-- And still stoppable.
	app:click('role:button "Open Running"')
	app:frames(30)
	app:click('role:button "Stop npm run dev"')
	app:frames(10)
	expect(app:state("calls")).to_contain("stop_task:s2:bg1")
	expect(app:error()).to_be(nil)
end)

test("a chat's subagents run in the Running tray, and a row opens its subagent", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	-- None yet: no pill.
	expect(app:has('role:button "Open Subagents"')).to_be(false)
	app:command("subagents_running")
	app:command("task_running")
	app:frames(30)
	-- Two agents and a process run: each its own, in the pill.
	expect(app:has('role:button "Open Subagents"')).to_be(true)
	expect(app:has('role:button "Open Running"')).to_be(true)
	app:click('role:button "Open Subagents"')
	app:frames(30)
	-- A row opens its subagent in the session's panel; an agent has no Stop,
	-- and the process is the other tab's.
	expect(app:has('role:button "Stop scan the repo"')).to_be(false)
	expect(app:has('role:button "Stop npm run dev"')).to_be(false)
	app:click('role:button "Open read the docs"')
	app:frames(5)
	expect(app:state("calls")).to_contain("open_subagent:s2:d2")
	-- The one that finished is folded apart, and opens from there.
	expect(app:has('role:button "Open find the flaky test"')).to_be(false)
	app:click('role:button "Show 1 finished"')
	app:frames(30)
	app:click('role:button "Open find the flaky test"')
	app:frames(5)
	expect(app:state("calls")).to_contain("open_subagent:s2:d3")
	expect(app:error()).to_be(nil)
end)

test("child chats a Zeron chat started run in the Subagents tray, as subagents do; the person's own side chats don't", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	expect(app:has('role:button "Open Subagents"')).to_be(false)
	app:command("child_chats")
	app:frames(30)
	-- The pill, as for subagents.
	expect(app:has('role:button "Open Subagents"')).to_be(true)
	app:click('role:button "Open Subagents"')
	app:frames(30)
	-- The working one is a row of its own; no Stop, as a subagent has none.
	expect(app:has('role:button "Open audit the parser"')).to_be(true)
	expect(app:has('role:button "Stop audit the parser"')).to_be(false)
	-- The one at rest is folded with the finished.
	expect(app:has('role:button "Open draft the release notes"')).to_be(false)
	app:click('role:button "Show 1 finished"')
	app:frames(30)
	expect(app:has('role:button "Open draft the release notes"')).to_be(true)
	-- The person's own side chat isn't here.
	expect(app:has('~"my own side chat"')).to_be(false)
	-- A row opens its chat in the panel, as a side chat's tab.
	app:click('role:button "Open audit the parser"')
	app:frames(5)
	expect(app:state("calls")).to_contain("open_lane:s2:side_chat:k1")
	app:click('role:button "Open draft the release notes"')
	app:frames(5)
	expect(app:state("calls")).to_contain("open_lane:s2:side_chat:k2")
	expect(app:state("calls")).never.to_contain(":k3")
	expect(app:error()).to_be(nil)
end)

test("child chats a Zeron chat started share the tray with the agent's own subagents", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:command("subagents_running")
	app:command("child_chats")
	app:frames(30)
	app:click('role:button "Open Subagents"')
	app:frames(30)
	for _, title in { "scan the repo", "read the docs", "audit the parser" } do
		expect(app:has(`role:button "Open {title}"`)).to_be(true)
	end
	-- Both finished fold together.
	expect(app:has('role:button "Show 2 finished"')).to_be(true)
	expect(app:has('~"my own side chat"')).to_be(false)
end)

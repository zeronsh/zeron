-- The agent's loaders (ds/agent_loader): its mark's tiles turning where the
-- main agent works (the reply's working line, a running tool, the activity
-- summary), the subagent's where a subagent or side chat runs (its row in
-- the transcript's lanes, the composer's Subagents tray), each there while
-- it works and gone when it's done.

local function eventually(app: any, ok: () -> boolean): boolean
	for _ = 1, 40 do
		if ok() then
			return true
		end
		app:frames(5)
	end
	return ok()
end

test("the agent's loader shows while a reply works, through its tools, and goes when it's done", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s1")
	app:frames(30)
	expect(app:has("#transcript >> .agent-loader-agent")).to_be(false)
	app:type('role:textbox "Message"', "Say something")
	app:key("Enter")
	app:frames(10)
	app:command("engine_copy")
	app:frames(10)
	app:command("turn_starts")
	-- Working: the reply's line.
	expect(eventually(app, function(): boolean
		return app:has("#transcript >> .agent-loader-agent")
	end)).to_be(true)
	-- A tool running, under the reply.
	app:command("start_reply")
	app:frames(10)
	app:command("add_tool")
	expect(eventually(app, function(): boolean
		return app:has("#transcript >> .agent-loader-agent")
	end)).to_be(true)
	-- Done: no loader left.
	app:command("end_reply")
	expect(eventually(app, function(): boolean
		return not app:has("#transcript >> .agent-loader-agent")
	end)).to_be(true)
	expect(app:has("#transcript >> .agent-loader-subagent")).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("a running subagent's row shows the subagent's loader, and loses it as it finishes", function()
	local app = mount("lanes_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	local row = '#session-lanes >> role:button "Open subagent scan the repo"'
	expect(app:has(`{row} >> .agent-loader-subagent`)).to_be(true)
	expect(app:has("#session-lanes >> .agent-loader-agent")).to_be(false)
	-- Finished: it folds away with the finished ones, its loader with it.
	app:command("finish")
	expect(eventually(app, function(): boolean
		return not app:has(`{row} >> .agent-loader-subagent`)
	end)).to_be(true)
	expect(app:has('role:button "Expand 2 finished subagents"')).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("a running subagent in the composer's Subagents tray shows the subagent's loader until it ends", function()
	local app = mount("running_tray_fixture.luau")
	app:resize(700, 500)
	app:frames(10)
	app:set_state("test_running_tasks", { { id = "a1", kind = "agent", name = "agent 1" } })
	app:frames(30)
	app:click('role:button "Open Subagents"')
	app:frames(30)
	expect(app:has(".agent-loader-subagent")).to_be(true)
	expect(app:has(".agent-loader-agent")).to_be(false)
	-- Ended: its glyph is the check, no loader.
	app:set_state("test_running_tasks", { { id = "a1", kind = "agent", name = "agent 1", ended = "done" } })
	expect(eventually(app, function(): boolean
		return not app:has(".agent-loader-subagent")
	end)).to_be(true)
	expect(app:error()).to_be(nil)
end)

-- What a chat leaves running once its turn is over (a background process,
-- subagents at work): with the draft empty, the composer's Send is a Stop
-- of all of it (screens/chat/background). Each is stopped on its host at
-- once, its row says "Stopping", and one its agent can't stop alone is said
-- in a toast without holding up the rest. Typing brings Send back; a turn's
-- own Stop stays the turn's. On ui/chat_fixture.luau.

local STOP_ALL = 'role:button "Stop background tasks and subagents"'

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

local function open(): any
	local app = mount("chat_fixture.luau")
	app:resize(1000, 800)
	app:command("go_s2")
	app:frames(30)
	return app
end

-- A process in the background and three subagents at work, no turn.
local function left_running(): any
	local app = open()
	app:command("task_running")
	app:command("subagents_running")
	app:frames(40)
	return app
end

test("with nothing running, an empty composer's Send is the usual, dimmed", function()
	local app = open()
	expect(app:has(STOP_ALL)).to_be(false)
	expect(app:tree('role:button "Send message"')).to_contain("disabled")
	expect(app:error()).to_be(nil)
end)

test("work left running with no turn: an empty composer's Send is a Stop of all of it", function()
	local app = left_running()
	expect(app:has(STOP_ALL)).to_be(true)
	expect(app:has('role:button "Send message"')).to_be(false)
	-- The turn's Stop isn't there: no turn runs.
	expect(app:has('role:button "Stop"')).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("typing brings Send back; emptied, the Stop of all of it returns", function()
	local app = left_running()
	app:type("role:textbox", "One more thing")
	app:frames(10)
	expect(app:has(STOP_ALL)).to_be(false)
	expect(app:has('role:button "Send message"')).to_be(true)
	expect(app:tree('role:button "Send message"')).never.to_contain("disabled")
	app:set_state("chat.composer.text", "")
	app:frames(10)
	expect(app:has(STOP_ALL)).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("its Stop stops every process and subagent at once, each row saying Stopping", function()
	local app = left_running()
	app:click('role:button "Open Running"')
	app:frames(30)
	app:click(STOP_ALL)
	app:frames(10)
	local c = calls(app)
	expect(c).to_contain("stop_task:s2:bg1")
	expect(c).to_contain("stop_call:s2:sp1")
	expect(c).to_contain("stop_call:s2:sp2")
	expect(c).to_contain("interrupt:kid1")
	-- (Never the turn: there is none.)
	expect(c).never.to_contain("interrupt:s2")
	-- The tray's row and the subagents' rows: Stopping, their Stops gone.
	expect(app:has('role:button "Stop npm run dev"')).to_be(false)
	expect(app:has('role:button "Stop Map the parser"')).to_be(false)
	expect(app:has('role:button "Stop Check the tests"')).to_be(false)
	expect(app:has('role:button "Stop Review the API"')).to_be(false)
	expect(app:tree()).to_contain('text "Stopping"')
	-- The composer's Stop waits, spinning, while they go.
	expect(app:has('role:button "Stopping background tasks and subagents"')).to_be(true)
	-- Once they've gone, Send again, dimmed.
	app:command("subagents_done")
	app:command("tasks_gone")
	app:frames(30)
	expect(app:has(STOP_ALL)).to_be(false)
	expect(app:has('role:button "Stopping background tasks and subagents"')).to_be(false)
	expect(app:tree('role:button "Send message"')).to_contain("disabled")
	expect(app:error()).to_be(nil)
end)

test("a subagent its agent can't stop alone is said in a toast, the rest still stopped", function()
	local app = left_running()
	app:command("stop_unsupported")
	app:click(STOP_ALL)
	app:frames(10)
	local c = calls(app)
	expect(c).to_contain("stop_task:s2:bg1")
	expect(c).to_contain("stop_call:s2:sp1")
	expect(c).to_contain("stop_call:s2:sp2")
	expect(c).to_contain("interrupt:kid1")
	expect(app:has('~"1 couldn\'t be stopped"')).to_be(true)
	expect(app:has('~"Map the parser: Not this tool."')).to_be(true)
	-- Its Stop is back; the others are stopping.
	expect(app:has('role:button "Stop Map the parser"')).to_be(true)
	expect(app:has('role:button "Stop Check the tests"')).to_be(false)
	-- And so is the composer's, for what's left.
	expect(app:has(STOP_ALL)).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("while a turn runs, its Stop stops the turn alone", function()
	local app = open()
	app:command("task_running")
	app:command("start_reply")
	app:frames(30)
	expect(app:has(STOP_ALL)).to_be(false)
	app:click('role:button "Stop"')
	app:frames(10)
	local c = calls(app)
	expect(c).to_contain("interrupt:s2")
	expect(c).never.to_contain("stop_task")
	expect(app:error()).to_be(nil)
end)

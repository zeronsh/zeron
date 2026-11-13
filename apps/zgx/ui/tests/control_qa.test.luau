-- The Control room's Q&A (ds/orch_qa on screens/control/page): every
-- question the person dumped, its answer from the thread it went to, read
-- and unread, put away, and the dump's question parts marked in Capture.
-- On ui/control_qa_fixture.luau (its header lists the entries). All of
-- them: `zgx test ui "Q&A:"`.

local function open(): any
	local app = mount("control_qa_fixture.luau")
	app:resize(1440, 1400)
	app:frames(30)
	return app
end

local QA = "#control-widget-qa"

local function marks(app: any): any
	return app:state("control_qa") or {}
end

test("Q&A: each question the person asked is an entry, newest first, put-away and non-questions left out", function()
	local app = open()
	expect(app:error()).to_be_nil()
	expect(app:has(QA)).to_be(true)
	expect(app:count(`{QA} >> .orch-qa-entry`)).to_be(5)
	-- The real one: asked in a dump, answered in a thread.
	expect(app:has(`{QA} >> role:button ~"Are Inbox and Next sections in the board ever used?"`)).to_be(true)
	-- Work in a question's shape, rhetoric, and the one put away: not here.
	expect(app:has(`{QA} >> ~"Why does the map flicker on resize?"`)).to_be(false)
	expect(app:has(`{QA} >> ~"so much better, right?"`)).to_be(false)
	expect(app:has(`{QA} >> ~"CI cache bucket"`)).to_be(false)
	-- Newest asked first: the sample's (40 s ago) before the real one (8 h).
	local tree = tostring(app:tree(QA))
	local moscow = string.find(tree, "moscow rebooted", 1, true) or 0
	local real = string.find(tree, "Inbox and Next sections", 1, true) or 0
	expect(moscow > 0 and real > moscow).to_be(true)
end)

test("Q&A: an entry says where it was answered, when it was asked and answered, or that it waits", function()
	local app = open()
	-- Where: a way into that thread, by its name.
	expect(app:has(`{QA} >> role:button "Open Explaining Capy.ai Captains"`)).to_be(true)
	expect(app:has(`{QA} >> ~"asked 8h"`)).to_be(true)
	expect(app:has(`{QA} >> ~"answered 7h"`)).to_be(true)
	-- Waiting: its thread at work on it, or not.
	expect(app:has(`{QA} >> .orch-qa-when >> "Answering"`)).to_be(true)
	expect(app:has(`{QA} >> .orch-qa-when >> "Waiting"`)).to_be(true)
	-- The head counts what's new and what waits.
	expect(app:has('#control-widget-qa >> .control-qa-new >> "2 new"')).to_be(true)
	expect(app:has('#control-widget-qa >> "2 waiting"')).to_be(true)
end)

test("Q&A: unread answers stand out; read ones step back", function()
	local app = open()
	-- The unread mark: the two answers not read yet.
	expect(app:count(`{QA} >> .orch-qa-unread`)).to_be(2)
	-- A preview of the answer, plain (its marks left out), never all of it.
	expect(app:has(`{QA} >> .orch-qa-preview >> ~"essentially nothing reaches Inbox or Next"`)).to_be(true)
	expect(app:has(`{QA} >> .orch-qa-preview >> ~"**"`)).to_be(false)
	expect(app:has(`{QA} >> .orch-qa-answer`)).to_be(false)
end)

test("Q&A: opening an entry shows its whole answer, rendered, and reads it", function()
	local app = open()
	app:click(`{QA} >> role:button ~"Are Inbox and Next sections"`)
	app:frames(10)
	expect(app:error()).to_be_nil()
	-- All of it, as markdown (its list items, its bold).
	expect(app:has(`{QA} >> .orch-qa-answer >> ~"Which do you want?"`)).to_be(true)
	expect(app:has(`{QA} >> .orch-qa-answer >> ~"Why you've never seen one:"`)).to_be(true)
	expect(marks(app).read["dp-qa/rq-1"]).to_be(true)
	expect(app:count(`{QA} >> .orch-qa-unread`)).to_be(1)
	-- Opened, kept so (a named state): closed again with another click.
	expect((app:state("control_qa_open") or {})["dp-qa/rq-1"]).to_be(true)
	app:click(`{QA} >> role:button ~"Are Inbox and Next sections"`)
	app:frames(10)
	expect(app:has(`{QA} >> .orch-qa-answer`)).to_be(false)
	-- Still read.
	expect(marks(app).read["dp-qa/rq-1"]).to_be(true)
end)

test("Q&A: the thread's link opens it and reads the answer", function()
	local app = open()
	app:click(`{QA} >> role:button "Open Explaining Capy.ai Captains"`)
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/th-capy")
	expect(marks(app).read["dp-qa/rq-1"]).to_be(true)
end)

test("Q&A: marked read and unread where the pointer rests", function()
	local app = open()
	local entry = `{QA} >> role:button ~"Are Inbox and Next sections"`
	app:hover(entry)
	app:frames(5)
	app:click(`{QA} >> role:button "Mark read"`)
	app:frames(5)
	expect(marks(app).read["dp-qa/rq-1"]).to_be(true)
	expect(app:count(`{QA} >> .orch-qa-unread`)).to_be(1)
	app:hover(entry)
	app:frames(5)
	app:click(`{QA} >> role:button "Mark unread"`)
	app:frames(5)
	expect(marks(app).read["dp-qa/rq-1"]).to_be_nil()
	expect(app:count(`{QA} >> .orch-qa-unread`)).to_be(2)
	-- A read one, made unread.
	app:hover(`{QA} >> role:button "When does 2.0 ship?"`)
	app:frames(5)
	app:click(`{QA} >> role:button "Mark unread"`)
	app:frames(5)
	expect(app:count(`{QA} >> .orch-qa-unread`)).to_be(3)
end)

test("Q&A: dismissed, an entry goes (and comes back with Undo)", function()
	local app = open()
	app:hover(`{QA} >> role:button "When does 2.0 ship?"`)
	app:frames(5)
	app:click(`{QA} >> role:button "Dismiss"`)
	app:frames(10)
	expect(marks(app).dismissed["dp-qa/rq-3"]).to_be(true)
	expect(app:count(`{QA} >> .orch-qa-entry`)).to_be(4)
	expect(app:has(`{QA} >> role:button "When does 2.0 ship?"`)).to_be(false)
	app:click('role:button "Undo"')
	app:frames(10)
	expect(marks(app).dismissed["dp-qa/rq-3"]).to_be_nil()
	expect(app:count(`{QA} >> .orch-qa-entry`)).to_be(5)
end)

test("Q&A: an answer arriving shows in place, unread, nothing rebuilt or moved", function()
	local app = open()
	app:command("fixture.qa_answer")
	app:frames(10)
	expect(app:error()).to_be_nil()
	expect(app:has(`{QA} >> .orch-qa-preview >> ~"signed this morning"`)).to_be(true)
	expect(app:count(`{QA} >> .orch-qa-unread`)).to_be(3)
	expect(app:count(`{QA} >> .orch-qa-entry`)).to_be(5)
	expect(app:has('#control-widget-qa >> "1 waiting"')).to_be(true)
end)

test("Q&A: a dump's question parts are marked in Capture; its other parts aren't", function()
	local app = open()
	-- The sample's newest dump: two questions (the VPN one, the engine state
	-- one), three other parts.
	expect(app:count("#control-capture >> .orch-question-word") > 0).to_be(true)
	expect(app:has('#control-capture >> .orch-question-word >> ~"moscow"')).to_be(true)
	expect(app:has('#control-capture >> .orch-question-word >> ~"registry"')).to_be(true)
	expect(app:has('#control-capture >> .orch-question-word >> ~"omarchy"')).to_be(false)
	expect(app:has('#control-capture >> .orch-question-word >> ~"tabs"')).to_be(false)
end)

test("Q&A: what's read and put away is kept (the device's), and the widget can be hidden", function()
	local app = open()
	expect(marks(app).read["dp-qa/rq-3"]).to_be(true)
	expect(marks(app).dismissed["dp-qa2/rq-6"]).to_be(true)
	-- In its place in the default order: under Needs you.
	local w = app:state("control_widgets") or {}
	local order = w.order or {}
	expect(table.find(order, "qa") == nil or table.find(order, "qa") == (table.find(order, "needs") or 0) + 1).to_be(true)
end)

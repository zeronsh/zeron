-- The Control room's Q&A (ds/orch_qa on screens/control/page): every
-- question the person dumped, its answer from the thread it went to, read
-- and unread, put away, and the dump's question parts marked in Capture.
-- On ui/control_qa_fixture.luau (its header lists the entries). All of
-- them: `zgx test ui "Q&A:"`.

local function open(): any
	local app = mount("control_qa_fixture.luau")
	app:resize(1440, 1400)
	-- (Q&A and Progress, under Notes.)
	app:set_state("control_tab", "notes")
	app:frames(30)
	return app
end

local QA = "#control-notes-qa"

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
	expect(app:has(`{QA} >> role:button "Go to Explaining Capy.ai Captains"`)).to_be(true)
	expect(app:has(`{QA} >> ~"asked 8h"`)).to_be(true)
	expect(app:has(`{QA} >> ~"answered 7h"`)).to_be(true)
	-- Waiting: its thread at work on it, or not.
	expect(app:has(`{QA} >> .orch-qa-when >> "Answering"`)).to_be(true)
	expect(app:has(`{QA} >> .orch-qa-when >> "Waiting"`)).to_be(true)
	-- The head counts what's new and what waits.
	expect(app:has('#control-notes-qa >> .control-qa-new >> "2 new"')).to_be(true)
	expect(app:has('#control-notes-qa >> "2 waiting"')).to_be(true)
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

test("Q&A: the thread's link opens it and reads the answer", function()
	local app = open()
	app:click(`{QA} >> role:button "Go to Explaining Capy.ai Captains"`)
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
	expect(app:has('#control-notes-qa >> "1 waiting"')).to_be(true)
end)

test("Q&A: a dump's question parts are marked in Capture; its other parts aren't", function()
	local app = open()
	-- (In Capture's history, opened from its bar.)
	app:click("#control-history-open")
	app:frames(30)
	-- The sample's newest dump: two questions (the VPN one, the engine state
	-- one), three other parts.
	expect(app:count("#control-capture >> .orch-question-word") > 0).to_be(true)
	expect(app:has('#control-capture >> .orch-question-word >> ~"moscow"')).to_be(true)
	expect(app:has('#control-capture >> .orch-question-word >> ~"registry"')).to_be(true)
	expect(app:has('#control-capture >> .orch-question-word >> ~"omarchy"')).to_be(false)
	expect(app:has('#control-capture >> .orch-question-word >> ~"tabs"')).to_be(false)
end)

test("Q&A: what's read and put away is kept (the device's), under Notes", function()
	local app = open()
	expect(marks(app).read["dp-qa/rq-3"]).to_be(true)
	expect(marks(app).dismissed["dp-qa2/rq-6"]).to_be(true)
	expect(app:has("#control-notes >> #control-notes-qa >> #orch-qa")).to_be(true)
end)

-- Loaded as the shell loads a screen (required inside a component's
-- build), then left and come back to: what the page made the first time
-- must still follow what changes.
local function screen(): any
	local app = mount("control_qa_screen_fixture.luau")
	app:resize(1440, 1400)
	-- (Q&A and Progress, under Notes.)
	app:set_state("control_tab", "notes")
	app:frames(30)
	app:set_state("qa_screen_shown", false)
	app:frames(10)
	app:set_state("qa_screen_shown", true)
	app:frames(30)
	return app
end

test("Q&A: after the screen is left and come back to, a dismissed entry goes at once and Undo brings it back", function()
	local app = screen()
	expect(app:error()).to_be_nil()
	expect(app:count(`{QA} >> .orch-qa-entry`)).to_be(5)
	app:hover(`{QA} >> role:button "When does 2.0 ship?"`)
	app:frames(5)
	app:click(`{QA} >> role:button "Dismiss"`)
	app:frames(5)
	expect(marks(app).dismissed["dp-qa/rq-3"]).to_be(true)
	expect(app:has(`{QA} >> role:button "When does 2.0 ship?"`)).to_be(false)
	expect(app:count(`{QA} >> .orch-qa-entry`)).to_be(4)
	-- Still gone with the screen made again.
	app:set_state("qa_screen_shown", false)
	app:frames(5)
	app:set_state("qa_screen_shown", true)
	app:frames(20)
	expect(app:has(`{QA} >> role:button "When does 2.0 ship?"`)).to_be(false)
	app:click('role:button "Undo"')
	app:frames(10)
	expect(app:has(`{QA} >> role:button "When does 2.0 ship?"`)).to_be(true)
end)

test("Q&A: after the screen is left and come back to, an answer arriving still shows", function()
	local app = screen()
	app:command("fixture.qa_answer")
	app:frames(10)
	expect(app:has(`{QA} >> .orch-qa-preview >> ~"signed this morning"`)).to_be(true)
end)

test("Progress: after the Control room is left and come back to, a thread that starts working shows", function()
	local app = screen()
	expect(app:has('#control-notes-progress >> ~"Brand new worker"')).to_be(false)
	app:command("fixture.new_worker")
	app:frames(20)
	expect(app:error()).to_be_nil()
	expect(app:has('#control-notes-progress >> ~"Brand new worker"')).to_be(true)
end)

test("Q&A: Answering only while the thread is on a turn begun since the question came; a busy thread not at it yet is Waiting", function()
	local app = open()
	-- The engine state thread: working, its turn begun after the question.
	expect(app:has(`{QA} >> role:button "is the engine state thread still on the registry part?"`)).to_be(true)
	local tree = tostring(app:tree(QA))
	local engine = string.find(tree, "registry part?", 1, true) or 0
	local msi = string.find(tree, "Windows MSI", 1, true) or 0
	local first = string.find(tree, "Answering", engine, true) or 0
	expect(first > engine and first < msi).to_be(true)
	-- The VPN orchestrator: working, but on a turn begun before the MSI
	-- question came.
	local after = string.sub(tree, msi)
	local waiting = string.find(after, "Waiting", 1, true)
	local answering = string.find(after, "Answering", 1, true)
	expect(waiting ~= nil).to_be(true)
	expect(answering == nil or answering > (waiting :: number)).to_be(true)
end)


-- ── The overlay (ds/orch_decision's Answer) ────────────────────────────────

local DIALOG = ".orch-qa-dialog"
local REAL = `{QA} >> role:button ~"Are Inbox and Next sections"`

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

test("Q&A: a click opens the entry whole in the overlay: question, answer, who answered and when, its thread and where it was asked", function()
	local app = open()
	app:click(REAL)
	app:frames(15)
	expect(app:error()).to_be_nil()
	expect(app:state("control_qa_shown")).to_be("dp-qa/rq-1")
	expect(app:has(DIALOG)).to_be(true)
	expect(app:has(`{DIALOG} >> .orch-decision-question >> ~"Are Inbox and Next sections in the board ever used?"`)).to_be(true)
	expect(app:has(`{DIALOG} >> .orch-qa-dialog-answer >> ~"Which do you want?"`)).to_be(true)
	expect(app:has(`{DIALOG} >> role:button "Go to Explaining Capy.ai Captains"`)).to_be(true)
	expect(app:has(`{DIALOG} >> .orch-qa-dialog-when >> ~"answered 7h ago"`)).to_be(true)
	expect(app:has(`{DIALOG} >> role:button "Where you asked"`)).to_be(true)
	expect(app:has(`{DIALOG} >> .orch-qa-dialog-state >> "Answer"`)).to_be(true)
	-- Opened is read; the row stays a preview (nothing opens in the column).
	expect(marks(app).read["dp-qa/rq-1"]).to_be(true)
	expect(app:has(`{QA} >> .orch-qa-answer`)).to_be(false)
	expect(app:has(`{QA} >> .orch-qa-preview >> ~"essentially nothing reaches Inbox or Next"`)).to_be(true)
	-- The thread, from it: the overlay closes and the thread opens.
	app:click(`{DIALOG} >> role:button "Go to Explaining Capy.ai Captains"`)
	app:frames(15)
	expect(app:state("shell_route_path")).to_be("/session/th-capy")
	expect(app:state("control_qa_shown")).to_be_nil()
end)

test("Q&A: the overlay renders the answer as markdown", function()
	local app = open()
	app:click(REAL)
	app:frames(15)
	-- Its bold and its lists as markdown draws them, never the marks.
	expect(app:has(`{DIALOG} >> .orch-qa-dialog-answer >> ~"Why you've never seen one:"`)).to_be(true)
	expect(app:has(`{DIALOG} >> .orch-qa-dialog-answer >> ~"the Brain dump routing a fragment straight to a board card"`)).to_be(true)
	expect(app:has(`{DIALOG} >> .orch-qa-dialog-answer >> ~"**"`)).to_be(false)
	expect(app:has(`{DIALOG} >> .orch-qa-dialog-answer >> ~"- **Inbox"`)).to_be(false)
end)

test("Q&A: a waiting entry opens too, saying it has no answer yet", function()
	local app = open()
	app:click(`{QA} >> role:button "Is the Windows MSI signed yet?"`)
	app:frames(15)
	expect(app:has(`{DIALOG} >> .orch-qa-dialog-none >> "No answer yet."`)).to_be(true)
	expect(app:has(`{DIALOG} >> .orch-qa-dialog-state >> "Waiting"`)).to_be(true)
	expect(app:has(`{DIALOG} >> role:button "Mark read"`)).to_be(false)
end)

test("Q&A: read and unread, and dismiss, from inside the overlay", function()
	local app = open()
	local vpn = `{QA} >> role:button "ask the vpn guy whether moscow rebooted fine"`
	app:click(vpn)
	app:frames(15)
	expect(marks(app).read["dp-2/rt-7"]).to_be(true)
	app:click(`{DIALOG} >> role:button "Mark unread"`)
	app:frames(5)
	expect(marks(app).read["dp-2/rt-7"]).to_be_nil()
	expect(app:has(`{DIALOG} >> .orch-qa-dialog-unread`)).to_be(true)
	app:click(`{DIALOG} >> role:button "Mark read"`)
	app:frames(5)
	expect(marks(app).read["dp-2/rt-7"]).to_be(true)
	app:click(`{DIALOG} >> role:button "Dismiss"`)
	app:frames(15)
	expect(marks(app).dismissed["dp-2/rt-7"]).to_be(true)
	expect(app:state("control_qa_shown")).to_be_nil()
	expect(app:has(vpn)).to_be(false)
	app:click('role:button "Undo"')
	app:frames(10)
	expect(app:has(vpn)).to_be(true)
end)

test("Q&A: a follow-up from the overlay goes to the same thread and comes back as a new entry", function()
	local app = open()
	app:click(REAL)
	app:frames(15)
	expect(app:count(`{QA} >> .orch-qa-entry`)).to_be(5)
	app:type(`{DIALOG} >> role:textbox`, "So which one would you wire up first?")
	app:key("Enter")
	app:frames(15)
	expect(app:error()).to_be_nil()
	-- The Q&A's ask: a dump carrying the entry, which the engine asks of
	-- the thread that answered it.
	expect(calls(app)).to_contain("dump:So which one would you wire up first?")
	expect(calls(app)).to_contain("dump-ref:qa:rq-1")
	local sent = app:state("last_dump") or {}
	expect(((sent.refs or {})[1] or {}).context.chat).to_be("th-capy")
	expect(app:state("control_qa_shown")).to_be_nil()
	app:command("fixture.qa_route_follow_up")
	app:frames(15)
	expect(app:count(`{QA} >> .orch-qa-entry`) >= 6).to_be(true)
	expect(app:has(`{QA} >> role:button "So which one would you wire up first?"`)).to_be(true)
end)

test("Q&A: Escape closes the overlay", function()
	local app = open()
	app:click(REAL)
	app:frames(15)
	expect(app:has(DIALOG)).to_be(true)
	app:key("Escape")
	app:frames(5)
	expect(app:state("control_qa_shown")).to_be_nil()
	-- (Hidden once it has eased out, as the decision's overlay.)
	local gone = false
	for _ = 1, 40 do
		app:frames(5)
		local first = if app:has(DIALOG) then (string.match(app:tree(DIALOG, 0), "^[^\n]*") or "") else "hidden"
		if string.find(first, "hidden", 1, true) then
			gone = true
			break
		end
	end
	expect(gone).to_be(true)
end)

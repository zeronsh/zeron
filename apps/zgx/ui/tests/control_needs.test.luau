-- Needs you (ds/orch_asks) in the Control room: a row a card in the
-- board's Needs you, an ask of no such card, a chat awaiting input that is
-- no such card's thread, urgent first; an ask answered in place goes to the
-- host and its row slides out. On ui/control_page_fixture.luau (the
-- sample's Needs you: the orchestrator screen's card asking ak-1, the
-- Moscow reboot's asking ak-2, sccache's blocked with nothing asked).
-- tests/control_needs_cards has a card asking two things.

local function open(): any
	local app = mount("control_page_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

local function top(app: any, what: string): number?
	for line in app:tree("#control-widget-needs"):gmatch("[^\n]+") do
		if line:find(what, 1, true) then
			return tonumber(line:match(" %-?%d+,(%-?%d+) "))
		end
	end
	return nil
end

test("what needs you lists the board's Needs you cards, urgent first, one row a card", function()
	local app = open()
	-- The orchestrator screen thread awaits input and its card has an ask
	-- open: one row, the card's, with its ask's options and a way into the
	-- chat. sccache's card, asking nothing, has its row too.
	expect(app:count("#control-widget-needs >> .orch-need")).to_be(3)
	expect(app:has('#control-widget-needs >> role:button "Open “sccache for iOS”"')).to_be(true)
	local urgent, decision = top(app, '"Approve"'), top(app, '"Side by side"')
	expect(urgent ~= nil and decision ~= nil and urgent < decision).to_be(true)
	expect(app:has('#control-widget-needs >> role:button "Open"')).to_be(true)
	expect(app:has('#control-widget-needs >> "Awaiting input"')).to_be(false)
end)

test("a waiting chat with no ask of its own and no card has a row to open it", function()
	local data = require("@ds/orch_data")
	local sample = require("@account/orchestration_sample")
	local now = table.clone(sample.now)
	now.asks = {}
	now.items = {}
	local needs = data.needs(now, { chats = sample.chats })
	expect(#needs).to_be(1)
	expect(needs[1].kind).to_be("input")
	-- Its card in Needs you: the card's row, opening the chat as well.
	local with_card = data.needs(sample.now, { chats = sample.chats })
	for _, n in with_card do
		expect(n.kind).to_be("card")
		if n.from == "th-ui-orch" then
			expect(n.input).to_be(true)
			expect(n.ask.id).to_be("ak-1")
		end
	end
end)

local function head(app: any): string
	return app:tree("#control-widget-needs >> .orch-block-head")
end

test("an option answers its ask after its Undo; once taken it says so, then leaves the list and the counts", function()
	local app = open()
	expect(head(app)).to_contain('"1 urgent"')
	app:click('#control-widget-needs >> role:button "Approve"')
	app:frames(4)
	-- On its way, Undo for a moment; nothing sent yet, and "Answered" not
	-- said before the engine has it. Counted out already.
	expect(app:has('#control-widget-needs >> .orch-sending >> "Approve"')).to_be(true)
	expect(app:has('#control-widget-needs >> role:button "Undo"')).to_be(true)
	expect(app:has('#control-widget-needs >> "Answered"')).to_be(false)
	expect(head(app)).never.to_contain("urgent")
	expect(calls(app)).never.to_contain("answer:")
	-- Past it: sent once, its option 1-based; the engine takes it.
	expect(app:wait('#control-widget-needs >> role:button "Undo"', { gone = true, timeout_ms = 8000 })).to_be(true)
	expect(app:wait('#control-widget-needs >> .orch-answered >> "Approve"', { timeout_ms = 2000 })).to_be(true)
	expect(calls(app)).to_contain("answer:ak-2:1:")
	-- (Still said while the engine's state, answered, has come.)
	app:frames(40)
	expect(app:has('#control-widget-needs >> .orch-answered >> "Answered"')).to_be(true)
	-- Then out of Needs you, and in Recent answers.
	expect(app:wait('#control-widget-needs >> .orch-need >> "Approve"', { gone = true, timeout_ms = 5000 })).to_be(true)
	app:frames(10)
	expect(app:count("#control-widget-needs >> .orch-need")).to_be(2)
	expect(head(app)).never.to_contain("urgent")
	app:click('#control-widget-needs >> role:button "Recent answers"')
	app:frames(6)
	expect(app:has('#control-widget-needs >> .orch-recent-row >> "Approve"')).to_be(true)
	local n = 0
	for _ in calls(app):gmatch("answer:") do
		n += 1
	end
	expect(n).to_be(1)
	expect(app:error()).to_be_nil()
end)

test("Undo takes a pick back: nothing is sent, and the chips and counts come back", function()
	local app = open()
	app:click('#control-widget-needs >> role:button "Approve"')
	app:frames(4)
	expect(head(app)).never.to_contain("urgent")
	app:click('#control-widget-needs >> role:button "Undo"')
	app:frames(4)
	expect(app:has('#control-widget-needs >> role:button "Approve"')).to_be(true)
	expect(head(app)).to_contain('"1 urgent"')
	expect(app:count("#control-widget-needs >> .orch-need")).to_be(3)
	app:frames(30)
	app:wait('#control-widget-needs >> "never there"', { timeout_ms = 6000 })
	expect(calls(app)).never.to_contain("answer:")
	expect(app:count("#control-widget-needs >> .orch-need")).to_be(3)
end)

test("an answer the engine turns down comes back with why, and Retry sends it again", function()
	local app = open()
	app:command("fixture.answers_refuse")
	app:click('#control-widget-needs >> role:button "Approve"')
	expect(app:wait('#control-widget-needs >> .orch-answer-error', { timeout_ms = 8000 })).to_be(true)
	app:frames(4)
	-- Never said answered; the chips back, why under them, counted again.
	expect(app:has('#control-widget-needs >> "Answered"')).to_be(false)
	expect(app:has('#control-widget-needs >> role:button "Approve"')).to_be(true)
	expect(app:has('#control-widget-needs >> "Couldn\'t send your answer: That ask was already answered or dismissed"')).to_be(true)
	expect(app:count("#control-widget-needs >> .orch-need")).to_be(3)
	expect(head(app)).to_contain('"1 urgent"')
	expect(calls(app)).to_contain("answer:ak-2:1:")
	-- Retry: sent again at once; taken this time.
	app:command("fixture.answers_ok")
	app:click('#control-widget-needs >> role:button "Retry"')
	app:frames(4)
	local n = 0
	for _ in calls(app):gmatch("answer:ak%-2:1:") do
		n += 1
	end
	expect(n).to_be(2)
	expect(app:wait('#control-widget-needs >> .orch-answered >> "Approve"', { timeout_ms = 2000 })).to_be(true)
	expect(app:has('#control-widget-needs >> .orch-answer-error')).to_be(false)
	expect(app:wait('#control-widget-needs >> .orch-need >> "Approve"', { gone = true, timeout_ms = 5000 })).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("an app without the answer action sends the answer's plain words through the op", function()
	local app = open()
	app:command("fixture.answers_no_host")
	app:click('#control-widget-needs >> role:button "Approve"')
	expect(app:wait('#control-widget-needs >> .orch-answered >> "Approve"', { timeout_ms = 8000 })).to_be(true)
	expect(calls(app)).to_contain("answer-op:ak-2:Approve")
	expect(calls(app)).never.to_contain("answer:")
	expect(app:has('#control-widget-needs >> .orch-answer-error')).to_be(false)
	expect(app:wait('#control-widget-needs >> .orch-need >> "Approve"', { gone = true, timeout_ms = 5000 })).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("an engine from before options is asked again with the plain words", function()
	local app = open()
	app:command("fixture.answers_old_engine")
	app:click('#control-widget-needs >> role:button "Approve"')
	expect(app:wait('#control-widget-needs >> .orch-answered >> "Approve"', { timeout_ms = 8000 })).to_be(true)
	-- The option first, refused as empty; then its label, as words.
	expect(calls(app)).to_contain("answer:ak-2:1:,answer:ak-2::Approve")
	expect(app:has('#control-widget-needs >> .orch-answer-error')).to_be(false)
end)

test("waiting for you leaves out an ask answered here", function()
	local data = require("@ds/orch_data")
	local now = {
		asks = {
			{ id = "a1", from_chat = "c1", kind = "approval", question = "Go?", options = {}, urgent = true, state = "open", created = 10 },
			{ id = "a2", from_chat = "c2", kind = "question", question = "?", options = {}, urgent = false, state = "open", created = 20 },
		},
	}
	local w = data.waiting_for_you(now, {}, nil, function(id: string): boolean
		return id ~= "a1"
	end)
	expect(w.count).to_be(1)
	expect(w.urgent).to_be(0)
end)

test("an ask is dismissed from its row", function()
	local app = open()
	app:click('#control-widget-needs >> role:button "Dismiss"[1]')
	app:frames(5)
	expect(calls(app)).to_contain("dismiss:ak-2")
end)

-- A thread that ended its turn with a plain question (an ask the engine
-- raised from its turn's end, control_world's `fixture.turn_end_ask`).
local PR = "Commit and open a PR against zeron/zgx"
local MERGE = "Merge into zeron/orchestrators"

local function asked(): any
	local app = open()
	app:command("fixture.turn_end_ask")
	app:frames(20)
	return app
end

test("a thread's question from its turn's end: the question, both options, whose and how long", function()
	local app = asked()
	-- (Its card is Running by its old column, so its ask is a row of its
	-- own: four.)
	expect(app:count("#control-widget-needs >> .orch-need")).to_be(4)
	-- (Its code spans drawn as code, their ticks gone.)
	expect(app:has('#control-widget-needs >> ~"Should I commit and open a PR against zeron/zgx, or merge it"')).to_be(true)
	expect(app:has('#control-widget-needs >> ~"`zeron/zgx`"')).to_be(false)
	expect(app:has(`#control-widget-needs >> role:button "{PR}"`)).to_be(true)
	expect(app:has(`#control-widget-needs >> role:button "{MERGE}"`)).to_be(true)
	-- Which thread asked (a way into it, at the question), how long it has waited.
	expect(app:has('#control-widget-needs >> role:button "Open Codex Slash Commands Missing at its question"')).to_be(true)
	expect(app:has('#control-widget-needs >> "2m"')).to_be(true)
	-- (Only the card whose chat awaits input, and sccache's asking nothing,
	-- have an Open of their own.)
	expect(app:count('#control-widget-needs >> role:button "Open"')).to_be(2)
	-- Compact: no reply line where there are options (the overlay has one).
	expect(app:has('#control-widget-needs >> role:textbox "Reply"')).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("a thread's turn-end ask: an option answers it", function()
	local app = asked()
	app:click(`#control-widget-needs >> role:button "{MERGE}"`)
	expect(app:wait('#control-widget-needs >> role:button "Undo"', { gone = true, timeout_ms = 8000 })).to_be(true)
	app:frames(4)
	expect(calls(app)).to_contain("answer:ak-turn:2:")
	expect(app:wait(`#control-widget-needs >> "{MERGE}"`, { gone = true, timeout_ms = 4000 })).to_be(true)
end)

test("a thread's turn-end ask: its name opens the thread at the message that asked", function()
	local app = asked()
	app:click('#control-widget-needs >> role:button "Open Codex Slash Commands Missing at its question"')
	app:frames(10)
	expect(calls(app)).to_contain("open:th-codex-slash:msg-turn-1")
end)

test("a chat awaiting input opens at its end", function()
	local app = open()
	-- (The orchestrator screen card's: under the Moscow reboot (urgent),
	-- over sccache's (in Needs you longer).)
	app:click('#control-widget-needs >> role:button "Open"[1]')
	app:frames(10)
	expect(calls(app)).to_contain("open:th-ui-orch:")
	expect(calls(app)).never.to_contain("open:th-ui-orch:m")
end)

test("a thread's turn-end ask: newest first, and gone stale, first of all", function()
	local app = asked()
	-- Newest first, as the board's column: asked after the decision's card
	-- came, over it; under the Moscow reboot, urgent.
	local before, decision, approve = top(app, `"{PR}"`), top(app, '"Side by side"'), top(app, '"Approve"')
	expect(before ~= nil and decision ~= nil and before < decision).to_be(true)
	expect(approve ~= nil and before ~= nil and (approve :: number) < (before :: number)).to_be(true)
	expect(app:count('#control-widget-needs >> "Urgent"')).to_be(1)
	app:command("fixture.turn_end_escalate")
	app:frames(30)
	-- Urgent now, and newer than the Moscow reboot: first.
	local after, decision2, approve2 = top(app, `"{PR}"`), top(app, '"Side by side"'), top(app, '"Approve"')
	expect(after ~= nil and approve2 ~= nil and (after :: number) < (approve2 :: number)).to_be(true)
	expect(approve2 ~= nil and decision2 ~= nil and (approve2 :: number) < (decision2 :: number)).to_be(true)
	expect(app:count('#control-widget-needs >> "Urgent"')).to_be(2)
end)

test("waiting for you counts open asks, chats awaiting input and unread Q&A answers once", function()
	local data = require("@ds/orch_data")
	local now = {
		orchestrators = {},
		crew = {},
		items = {},
		dumps = {},
		asks = {
			{ id = "a1", from_chat = "c1", kind = "decision", question = "A or B?", options = {}, urgent = false, state = "open", created = 10, origin = "turn_end" },
			{ id = "a2", from_chat = "c2", kind = "question", question = "?", options = {}, urgent = false, state = "open", created = 20, escalated = 40 },
			{ id = "a3", from_chat = "c3", kind = "question", question = "?", options = {}, urgent = false, state = "answered", created = 5 },
		},
	}
	local w = data.waiting_for_you(now, {})
	expect(w.count).to_be(2)
	expect(w.urgent).to_be(1)
	expect(w.qa_unread).to_be(0)
	local with_qa = data.waiting_for_you(now, {}, { unread = 3, waiting = 1 })
	expect(with_qa.count).to_be(5)
	expect(#with_qa.needs).to_be(2)
end)

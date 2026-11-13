-- Needs you (ds/orch_asks) in the Control room: asks and chats awaiting
-- input, urgent first; an ask answered in place goes to the host and its
-- row slides out. On ui/control_page_fixture.luau.

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

test("what needs you lists asks and waiting chats, urgent first, one row a chat", function()
	local app = open()
	-- The orchestrator screen thread awaits input and has an ask open: one
	-- row, the ask's, with its options and a way into the chat.
	expect(app:count("#control-widget-needs >> .orch-need")).to_be(2)
	local urgent, decision = top(app, '"Approve"'), top(app, '"Side by side"')
	expect(urgent ~= nil and decision ~= nil and urgent < decision).to_be(true)
	expect(app:has('#control-widget-needs >> role:button "Open"')).to_be(true)
	expect(app:has('#control-widget-needs >> "Awaiting input"')).to_be(false)
end)

test("a waiting chat with no ask of its own has a row to open it", function()
	local data = require("@ds/orch_data")
	local sample = require("@account/orchestration_sample")
	local now = table.clone(sample.now)
	now.asks = {}
	local needs = data.needs(now, { chats = sample.chats })
	expect(#needs).to_be(1)
	expect(needs[1].kind).to_be("input")
	local with_ask = data.needs(sample.now, { chats = sample.chats })
	for _, n in with_ask do
		expect(n.kind).to_be("ask")
		if n.from == "th-ui-orch" then
			expect(n.input).to_be(true)
		end
	end
end)

test("an option answers its ask after its Undo, and the row slides out", function()
	local app = open()
	app:click('#control-widget-needs >> role:button "Approve"')
	app:frames(4)
	-- Answered at once, Undo for a moment; nothing sent yet.
	expect(app:has('#control-widget-needs >> .orch-answered >> "Approve"')).to_be(true)
	expect(app:has('#control-widget-needs >> role:button "Undo"')).to_be(true)
	expect(calls(app)).never.to_contain("answer:")
	-- Past it: sent once, its option 1-based, and the row goes.
	expect(app:wait('#control-widget-needs >> role:button "Undo"', { gone = true, timeout_ms = 8000 })).to_be(true)
	app:frames(4)
	expect(calls(app)).to_contain("answer:ak-2:1:")
	expect(app:wait('#control-widget-needs >> "Approve"', { gone = true, timeout_ms = 4000 })).to_be(true)
	expect(app:count("#control-widget-needs >> .orch-need")).to_be(1)
	local n = 0
	for _ in calls(app):gmatch("answer:") do
		n += 1
	end
	expect(n).to_be(1)
end)

test("Undo takes a pick back: nothing is sent and the chips come back", function()
	local app = open()
	app:click('#control-widget-needs >> role:button "Not tonight"')
	app:frames(4)
	app:click('#control-widget-needs >> role:button "Undo"')
	app:frames(4)
	expect(app:has('#control-widget-needs >> role:button "Approve"')).to_be(true)
	app:frames(30)
	app:wait('#control-widget-needs >> "never there"', { timeout_ms = 6000 })
	expect(calls(app)).never.to_contain("answer:")
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
	expect(app:count("#control-widget-needs >> .orch-need")).to_be(3)
	-- (Its code spans drawn as code, their ticks gone.)
	expect(app:has('#control-widget-needs >> ~"Should I commit and open a PR against zeron/zgx, or merge it"')).to_be(true)
	expect(app:has('#control-widget-needs >> ~"`zeron/zgx`"')).to_be(false)
	expect(app:has(`#control-widget-needs >> role:button "{PR}"`)).to_be(true)
	expect(app:has(`#control-widget-needs >> role:button "{MERGE}"`)).to_be(true)
	-- Which thread asked (a way into it, at the question), how long it has waited.
	expect(app:has('#control-widget-needs >> role:button "Open Codex Slash Commands Missing at its question"')).to_be(true)
	expect(app:has('#control-widget-needs >> "2m"')).to_be(true)
	-- (Only the chat awaiting input has an Open of its own.)
	expect(app:count('#control-widget-needs >> role:button "Open"')).to_be(1)
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
	app:click('#control-widget-needs >> role:button "Open"')
	app:frames(10)
	expect(calls(app)).to_contain("open:th-ui-orch:")
	expect(calls(app)).never.to_contain("open:th-ui-orch:m")
end)

test("a thread's turn-end ask gone stale comes first, urgent", function()
	local app = asked()
	-- Asked after the decision was: under it, while neither is urgent.
	local before, decision = top(app, `"{PR}"`), top(app, '"Side by side"')
	expect(before ~= nil and decision ~= nil and decision < before).to_be(true)
	expect(app:count('#control-widget-needs >> "Urgent"')).to_be(1)
	app:command("fixture.turn_end_escalate")
	app:frames(30)
	-- Urgent now: over the decision, under the Moscow reboot (urgent, and
	-- waiting longer).
	local after, decision2, approve = top(app, `"{PR}"`), top(app, '"Side by side"'), top(app, '"Approve"')
	expect(after ~= nil and decision2 ~= nil and after < decision2).to_be(true)
	expect(approve ~= nil and after ~= nil and (approve :: number) < (after :: number)).to_be(true)
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

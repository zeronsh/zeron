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

test("an option answers its ask, and the row slides out", function()
	local app = open()
	app:click('#control-widget-needs >> role:button "Approve"')
	app:frames(2)
	expect(calls(app)).to_contain("answer:ak-2:Approve")
	-- Answered once, however fast the second click.
	app:frames(60)
	expect(app:wait('#control-widget-needs >> role:button "Approve"', { gone = true, timeout_ms = 3000 })).to_be(true)
	expect(app:count("#control-widget-needs >> .orch-need")).to_be(1)
	local n = 0
	for _ in calls(app):gmatch("answer:") do
		n += 1
	end
	expect(n).to_be(1)
end)

test("an ask is dismissed from its row", function()
	local app = open()
	app:click('#control-widget-needs >> role:button "Dismiss"[1]')
	app:frames(5)
	expect(calls(app)).to_contain("dismiss:ak-2")
end)

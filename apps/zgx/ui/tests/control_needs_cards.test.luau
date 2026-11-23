-- Needs you counts cards (ds/orch_data `needs`, built on the board's
-- Needs you): a row a card in that column (its first ask, how many more on
-- it), a row an open ask not put off that belongs to no such card, a row a
-- chat awaiting input that is no such card's thread. Needs you floating
-- over the Map (every row opened) and the sidebar's badge read that one
-- list. On
-- ui/control_needs_cards_fixture.luau (ui/control_world's sample, plus a
-- second ask on the Orchestrator screen card, an ask of CI speed's on no
-- card and one put off until tomorrow).

local FLOAT = "#control-needs-float"
-- (Its rows: the float, on the Map, every row opened.)
local W = FLOAT

local function open(): any
	local app = mount("control_needs_cards_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	app:command("fx.map")
	app:frames(30)
	app:click(`{FLOAT} >> role:button "3 more"`)
	app:frames(30)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

-- The row whose tree holds `what`, as text.
local function row_of(app: any, scope: string, what: string): string?
	local n = app:count(`{scope} >> .orch-need`)
	for i = 1, n do
		local t = app:tree(`{scope} >> .orch-need[{i}]`)
		if t:find(what, 1, true) then
			return t
		end
	end
	return nil
end

test("needs cards: a card asking two things is one row, its first ask and 1 more", function()
	local app = open()
	local row = row_of(app, W, "Side by side")
	expect(row ~= nil).to_be(true)
	expect(row).to_contain('"1 more"')
	expect(row).to_contain('"Orchestrator screen"')
	-- (Its second ask isn't a row of its own.)
	expect(app:has(`{W} >> ~"Keep the crew list under the board"`)).to_be(false)
	expect(app:count(`{W} >> .orch-need-more`)).to_be(1)
	expect(app:error()).to_be_nil()
end)

test("needs cards: its first ask answered here, the row stays and shows the next", function()
	local app = open()
	app:click(`{W} >> role:button "Side by side"`)
	expect(app:wait(`{W} >> role:button "Undo"`, { gone = true, timeout_ms = 8000 })).to_be(true)
	expect(app:wait(`{W} >> role:button "Under the board"`, { timeout_ms = 6000 })).to_be(true)
	app:frames(30)
	expect(calls(app)).to_contain("answer:ak-1:1:")
	local row = row_of(app, W, "Under the board")
	expect(row ~= nil).to_be(true)
	expect(row).to_contain('"Orchestrator screen"')
	expect(app:has(`{W} >> .orch-need-more`)).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("needs cards: an ask that belongs to no Needs-you card is a row of its own", function()
	local app = open()
	local row = row_of(app, W, "Pause the nightly iOS lane")
	expect(row ~= nil).to_be(true)
	expect(row).to_contain('"CI speed"')
	expect(row).never.to_contain("more")
	expect(app:error()).to_be_nil()
end)

test("needs cards: an ask put off has no row", function()
	local app = open()
	expect(app:has(`{W} >> ~"Rotate the exit keys"`)).to_be(false)
	local data = require("@ds/orch_data")
	local t = os.time()
	local now = {
		items = {},
		asks = {
			{ id = "a-later", from_chat = "c1", kind = "question", question = "Later?", options = {}, state = "open", created = t - 10, deferred_until = t + 3600 },
			{ id = "a-now", from_chat = "c2", kind = "question", question = "Now?", options = {}, state = "open", created = t - 10 },
		},
	}
	local needs = data.needs(now, {})
	expect(#needs).to_be(1)
	expect(needs[1].id).to_be("ask:a-now")
end)

test("needs cards: a chat awaiting input that is a Needs-you card's thread is no extra row", function()
	local app = open()
	-- (th-ui-orch, the Orchestrator screen card's thread, awaits input: its
	-- card's row opens it.)
	local row = row_of(app, W, "Side by side") or ""
	expect(row).to_contain('button "Open"')
	local data = require("@ds/orch_data")
	local now = {
		items = { { id = "it-a", title = "A card", state = "blocked", threads = { "th-a" }, updated = 5 } },
		asks = { { id = "a1", from_chat = "th-a", item_id = "it-a", kind = "question", question = "?", options = {}, state = "open", created = 10 } },
	}
	local sessions = { chats = {
		{ id = "th-a", title = "Its thread", status = "input" },
		{ id = "th-b", title = "Another", status = "input" },
	} }
	local needs = data.needs(now, sessions)
	local ids = {}
	for _, n in needs do
		table.insert(ids, n.id)
	end
	table.sort(ids)
	expect(table.concat(ids, ",")).to_be("card:it-a,chat:th-b")
	for _, n in needs do
		if n.kind == "card" then
			expect(n.input).to_be(true)
			expect(n.ask.id).to_be("a1")
		end
	end
end)

test("needs cards: the badge's count is the widget's rows, the board's Needs you and the asks of no card", function()
	local app = open()
	local data = require("@ds/orch_data")
	local model = require("@ds/orch_board_model")
	local now, sessions = app:state("orchestration"), app:state("sessions")
	local board = model.board(now)
	local rows = app:count(`{W} >> .orch-need`)
	-- Three cards in the column, one ask of no card.
	expect(board.counts.needs_you).to_be(3)
	expect(rows).to_be(4)
	-- (The sidebar's badge: screens/shell/sidebar `needs_you`.)
	expect(#data.waiting_for_you(now, sessions).needs).to_be(rows)
	local cards = 0
	for _, n in data.needs(now, sessions) do
		if n.kind == "card" then
			cards += 1
		end
	end
	expect(cards).to_be(board.counts.needs_you)
	-- Every card in the column has its row.
	for _, e in board.needs_you do
		expect(app:has(`{W} >> .orch-need >> ~"{e.item.title}"`)).to_be(true)
	end
	expect(app:tree(FLOAT)).to_contain('"4"')
	expect(app:error()).to_be_nil()
end)

test("needs cards: with no ask of no card, the badge is the board's Needs you count", function()
	local data = require("@ds/orch_data")
	local model = require("@ds/orch_board_model")
	local now = {
		items = {
			{ id = "it-a", title = "A", state = "blocked", threads = { "th-a" }, updated = 5 },
			{ id = "it-b", title = "B", state = "review", threads = { "th-b" }, updated = 6 },
			{ id = "it-c", title = "C", state = "active", threads = { "th-c" }, updated = 7 },
		},
		asks = {
			{ id = "a1", from_chat = "th-a", item_id = "it-a", kind = "question", question = "?", options = {}, state = "open", created = 10 },
			{ id = "a2", from_chat = "th-a", item_id = "it-a", kind = "question", question = "??", options = {}, state = "open", created = 11 },
			{ id = "a3", from_chat = "th-b", item_id = "it-b", kind = "approval", question = "Look?", options = {}, state = "open", created = 12, urgent = true },
		},
	}
	local board = model.board(now)
	local w = data.waiting_for_you(now, {})
	expect(board.counts.needs_you).to_be(2)
	expect(#w.needs).to_be(2)
	expect(w.urgent).to_be(1)
	-- Both of a card's asks answered here: its row isn't counted; one of
	-- them: it is.
	local both = data.waiting_for_you(now, {}, nil, function(id: string): boolean
		return id ~= "a1" and id ~= "a2"
	end)
	expect(#both.needs).to_be(1)
	local one = data.waiting_for_you(now, {}, nil, function(id: string): boolean
		return id ~= "a1"
	end)
	expect(#one.needs).to_be(2)
	-- The same list again while nothing changed (one build for every reader).
	local s = {}
	expect(data.needs(now, s) == data.needs(now, s)).to_be(true)
end)

test("needs cards: the float counts the same rows, and isn't on the Board", function()
	local app = open()
	expect(app:has(FLOAT)).to_be(true)
	expect(app:tree(FLOAT)).to_contain('"4"')
	app:command("fx.board")
	app:frames(30)
	expect(app:has(FLOAT)).to_be(false)
	expect(app:error()).to_be_nil()
end)

-- The top of the first node under `scope` whose line holds `what`.
local function top(app: any, scope: string, what: string): number?
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(what, 1, true) then
			return tonumber(line:match(" %-?[%d%.]+,(%-?[%d%.]+) "))
		end
	end
	return nil
end

-- `names` in the order they stand under `scope`, top to bottom.
local function order(app: any, scope: string, names: { string }): string
	local placed = {}
	for _, n in names do
		local y = top(app, scope, `"{n}"`)
		expect(y ~= nil).to_be(true)
		table.insert(placed, { n = n, y = y :: number })
	end
	table.sort(placed, function(a: any, b: any): boolean
		return a.y < b.y
	end)
	local out = {}
	for _, p in placed do
		table.insert(out, p.n)
	end
	return table.concat(out, ", ")
end

-- Urgent first (the Moscow reboot), then the newest: the orchestrator
-- screen's card (came 3m ago), CI speed's ask of no card (15m), sccache's
-- card (in Needs you for a day).
local ROWS = { "Reboot Moscow node", "Orchestrator screen", "CI speed", "sccache for iOS" }
local CARDS = { "Reboot Moscow node", "Orchestrator screen", "sccache for iOS" }

test("needs cards: one order in the float and the column, urgent first then newest", function()
	local app = open()
	local want = table.concat(ROWS, ", ")
	expect(order(app, W, ROWS)).to_be(want)
	-- The data's own order, and its cards in the board's.
	local data = require("@ds/orch_data")
	local model = require("@ds/orch_board_model")
	local now, sessions = app:state("orchestration"), app:state("sessions")
	local cards, column = {}, {}
	for _, n in data.needs(now, sessions) do
		if n.kind == "card" then
			table.insert(cards, n.card)
		end
	end
	for _, e in model.board(now).needs_you do
		table.insert(column, e.id)
	end
	expect(table.concat(cards, ",")).to_be(table.concat(column, ","))
	-- The Board's Needs you column: its cards in the same order.
	app:command("fx.board")
	app:frames(30)
	expect(order(app, "#orch-col-needs_you", CARDS)).to_be(table.concat(CARDS, ", "))
	expect(app:error()).to_be_nil()
end)

test("needs cards: in the narrow float, the card's title whole and 1 more under its question", function()
	local app = open()
	-- (Its title as it is, not cut.)
	expect(app:has(`{FLOAT} >> "Orchestrator screen"`)).to_be(true)
	local head = top(app, FLOAT, '"Orchestrator screen"') :: number
	local question = top(app, FLOAT, '"Orchestrator page: conversation') :: number
	local more = top(app, FLOAT, '"1 more"') :: number
	local chip = top(app, FLOAT, 'button "Side by side"') :: number
	expect(head ~= nil and question ~= nil and more ~= nil and chip ~= nil).to_be(true)
	expect(head < question).to_be(true)
	expect(question < more).to_be(true)
	expect(more < chip).to_be(true)
	expect(app:error()).to_be_nil()
end)

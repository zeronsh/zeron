-- Every list the Control room and an orchestrator's page show that grows
-- with the work is capped, the rest a click away (ui/control_fleet_fixture:
-- a fleet as big as a person's, or a small one):
--
--   Needs you (floating over the Map, Chat and Notes; an orchestrator's
--   side panel)   chats awaiting input with nothing to show are one line;
--                 "n more" opens the rest in a list that scrolls past
--                 about ten rows
--   Progress (Notes), Q&A   "n more" opens the rest in a list that scrolls
--
-- and a small fleet shows what it did before.

local FLEET = "control_fleet_fixture.luau"

local function open(set: string, w: number?): any
	local app = mount(FLEET)
	app:resize(w or 1600, 1000)
	app:frames(20)
	app:command(set)
	app:set_state("control_tab", "map")
	app:frames(60)
	return app
end

-- An element's box: x, y, w, h.
local function box(app: any, target: string): (number, number, number, number)
	local line = string.match(app:tree(target, 0), "[^\n]+") or ""
	local x, y, w, h = string.match(line, " (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
	return tonumber(x) or 0, tonumber(y) or 0, tonumber(w) or 0, tonumber(h) or 0
end

-- Every chat of the fleet, by id (crew threads and side chats too).
local function chats(app: any): { [string]: any }
	local s = app:state("sessions") :: any
	local out = {}
	for _, c in s.chats or {} do
		out[c.id] = c
	end
	for _, list in s.children or {} do
		for _, c in list do
			out[c.id] = c
		end
	end
	return out
end

-- Needs you ----------------------------------------------------------------

-- (Needs you floating over the Map: its top ask, the rest on "n more".)
local NEEDS = "#control-needs-float"

-- The opened list scrolled down to the chats-awaiting-input line: it
-- stands where the newest of them comes in the order (urgent first, then
-- newest, as the board's column), under the cards and asks that came
-- after it.
local function to_inputs(app: any)
	-- (The float's own scroll: its list stands whole inside it.)
	app:scroll(NEEDS, 100000)
	app:frames(20)
end

-- The chats awaiting input with no ask of theirs: what the line counts.
local function bare_inputs(app: any): number
	local asking = {}
	for _, a in (app:state("orchestration") :: any).asks or {} do
		if a.state == "open" then
			asking[a.from_chat] = true
		end
	end
	local n = 0
	for id, c in chats(app) do
		if (c.status == "input" or c.status == "approval") and not c.archived and not asking[id] then
			n += 1
		end
	end
	return n
end

test("list caps: Needs you folds chats awaiting input into one line, which opens", function()
	local app = open("fleet_dense")
	local n = bare_inputs(app)
	expect(n > 1).to_be(true)
	app:click(`{NEEDS} >> .orch-asks-more`)
	app:frames(30)
	expect(app:count(`{NEEDS} >> .orch-need-inputs`)).to_be(1)
	expect(app:has(`{NEEDS} >> .orch-need-inputs >> ~"{n} chats waiting for input"`)).to_be(true)
	-- (Folded: none of their rows.)
	expect(app:has(`{NEEDS} >> .orch-need-inputs >> .orch-need`)).to_be(false)
	to_inputs(app)
	app:click(`{NEEDS} >> .orch-need-inputs-toggle`)
	app:frames(30)
	expect(app:count(`{NEEDS} >> .orch-need-inputs >> .orch-need`)).to_be(n)
end)

test("list caps: Needs you's n more opens the rest in a list that scrolls past ten rows", function()
	local app = open("fleet_dense")
	-- (Its top row alone, folded.)
	local before = app:count(`{NEEDS} >> .orch-need`) + app:count(`{NEEDS} >> .orch-need-inputs`)
	expect(before).to_be(1)
	expect(app:has(`{NEEDS} >> .orch-asks-more >> ~"more"`)).to_be(true)
	local _, _, _, closed = box(app, NEEDS)
	app:click(`{NEEDS} >> .orch-asks-more`)
	app:frames(40)
	local _, _, _, list_h = box(app, `{NEEDS} >> .orch-asks-list [1]`)
	local _, _, _, opened = box(app, NEEDS)
	-- Ten rows or so, not every one: the list scrolls.
	local rows = app:count(`{NEEDS} >> .orch-asks-list >> .orch-need`) + app:count(`{NEEDS} >> .orch-need-inputs`)
	expect(rows > 10).to_be(true)
	expect(list_h < 10 * 140).to_be(true)
	expect(opened - closed < 10 * 140).to_be(true)
	expect(app:error()).to_be_nil()
end)

-- An orchestrator's page has no Needs you block of its own any more: its
-- side panel's Tasks list has a Needs you section, a row a card, built only
-- near the view and capped at its own "Show n more" (ds/orch_board_list,
-- tests/orch_board_cheap). What it lists is the board's column for that
-- orchestrator, the Control room's card rows among them.
test("list caps: an orchestrator's Needs you is its board column, a row a card", function()
	local app = open("fleet_dense")
	app:command("fleet_orch_orc-ui")
	app:frames(90)
	local model = require("@ds/orch_board_model")
	local board = model.board(app:state("orchestration"), { filter = "orc-ui" })
	expect(#board.needs_you > 0).to_be(true)
	for _, e in board.needs_you do
		expect(app:has(`#orch-list-needs_you >> #orch-row-{e.id}`)).to_be(true)
	end
	expect(app:tree("#orch-list-needs_you", 1)).to_contain(`text "{#board.needs_you}"`)
	expect(app:error()).to_be_nil()
end)

test("list caps: a small fleet's Needs you is as it was", function()
	local app = open("fleet_sparse")
	expect(app:count(`{NEEDS} >> .orch-need`)).to_be(1)
	expect(app:has(`{NEEDS} >> .orch-need-inputs`)).to_be(false)
	expect(app:has(`{NEEDS} >> .orch-asks-more`)).to_be(false)
end)

test("list caps: Progress's n more opens the rest in a list that scrolls", function()
	local app = open("fleet_dense")
	app:set_state("control_tab", "notes")
	app:frames(60)
	-- (Needs you floating, minimised: out of the way of the column's foot.)
	app:click("#control-needs-min")
	app:frames(20)
	expect(app:has("#control-progress-more")).to_be(true)
	-- (Its "n more" brought into view: ten units stand above it.)
	local _, top, _, view_h = box(app, "#control-notes-scroll")
	local _, y = box(app, "#control-progress-more")
	app:set_state("control_scroll", math.max(0, y - top - view_h + 80))
	app:frames(40)
	local _, _, _, closed = box(app, "#control-notes-progress")
	app:click("#control-progress-more")
	app:frames(40)
	local _, _, _, opened = box(app, "#control-notes-progress")
	local _, _, _, list_h = box(app, ".control-progress-list [1]")
	expect(list_h <= 800).to_be(true)
	expect(opened - closed <= 800).to_be(true)
end)

test("list caps: the chats awaiting input, opened, scroll past ten", function()
	local app = open("fleet_dense")
	local n = bare_inputs(app)
	expect(n > 10).to_be(true)
	app:click(`{NEEDS} >> .orch-asks-more`)
	app:frames(30)
	to_inputs(app)
	app:click(`{NEEDS} >> .orch-need-inputs-toggle`)
	app:frames(30)
	local _, _, _, h = box(app, `{NEEDS} >> .orch-need-inputs-list [1]`)
	expect(h <= 10 * 52).to_be(true)
end)

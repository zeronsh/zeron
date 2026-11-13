-- The board (ds/orch_board, ui/orch_board_fixture.luau) on the sample:
-- its columns and what's in each, priority first, each column as wide as
-- the next; a card dragged to another column or within one says where (its
-- state and an order between its new neighbours); its menu sets a
-- priority; an outcome is two quiet lines; whose a card is shows only
-- while the board shows several orchestrators' cards; an empty board is
-- one line; and it holds side by side when wide, scrolls sideways when
-- narrower, stacks when narrow. (Raw pointer drags, fast and slow, Escape
-- and click: src/orchestration_board_tests.rs.)

local function board(w: number?, h: number?): any
	local app = mount("orch_board_fixture.luau")
	app:resize(w or 1400, h or 900)
	app:frames(20)
	return app
end

-- The cards' titles in a column, in order.
local function titles(app: any, col: string): { string }
	local out: { string } = {}
	for line in app:tree(`#orch-col-{col}`):gmatch("[^\n]+") do
		-- (A card is a button right under its column; its own buttons, its
		-- threads' dots and its chips, sit deeper.)
		local t = line:match('^  @%d+ button "([^"]+)"')
		if t then
			table.insert(out, t)
		end
	end
	return out
end

local function count(app: any, col: string): number
	return #titles(app, col)
end

-- A node's box as the tree says it: x, y, w, h.
local function box(app: any, target: string): (number, number, number, number)
	local first = app:tree(target):match("[^\n]+") or ""
	local x, y, w, h = first:match("(%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)%s*$")
	return tonumber(x) or 0, tonumber(y) or 0, tonumber(w) or 0, tonumber(h) or 0
end

local COLS = { "now", "next", "blocked", "review", "done" }

test("orch board: the sample's columns, priority first", function()
	local app = board()
	expect(count(app, "now")).to_be(4)
	expect(count(app, "next")).to_be(2)
	expect(count(app, "blocked")).to_be(2)
	expect(count(app, "review")).to_be(1)
	expect(count(app, "done")).to_be(1)
	-- Urgent first, then high by order, then normal.
	local now = titles(app, "now")
	expect(now[1]).to_be("Control room")
	expect(now[2]).to_be("Engine: orchestration state + RPC")
	expect(now[3]).to_be("Reports, asks, MCP tools")
	expect(now[4]).to_be("Merge phase-1 fixes")
	-- Progress from its threads' todo (Control room's 2 of 7, as cells and
	-- its count; Moscow's, all three of three).
	expect(app:has('#orch-col-now >> "2/7"')).to_be(true)
	expect(app:has("#orch-col-now >> .orch-card-progress")).to_be(true)
	expect(app:has('#orch-col-review >> "3/3"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch board: every column as wide as the next, heads on one line", function()
	for _, size in { { 1400, 900 }, { 1100, 800 } } do
		local app = board(size[1], size[2])
		local _, y0, w0, h0 = box(app, "#orch-col-now")
		expect(w0 > 150).to_be(true)
		for _, c in COLS do
			local _, y, w, h = box(app, `#orch-col-{c}`)
			expect(math.abs(w - w0) < 1).to_be(true)
			-- (Lanes side by side, as tall as the tallest, their heads level.)
			expect(math.abs(y - y0) < 1).to_be(true)
			expect(math.abs(h - h0) < 1).to_be(true)
		end
		-- An empty column keeps its width: Review emptied by the filter.
		app:set_state("filter", "orc-orch")
		app:frames(10)
		expect(count(app, "review")).to_be(0)
		local _, _, wr = box(app, "#orch-col-review")
		local _, _, wn = box(app, "#orch-col-now")
		expect(math.abs(wr - wn) < 1).to_be(true)
	end
end)

test("orch board: filtered to one orchestrator", function()
	local app = board()
	app:set_state("filter", "orc-orch")
	app:frames(10)
	expect(count(app, "now")).to_be(3)
	expect(count(app, "next")).to_be(1)
	expect(count(app, "blocked")).to_be(1)
	expect(count(app, "review")).to_be(0)
	-- Its chips set it too.
	app:click('role:button "All"')
	app:frames(10)
	expect(count(app, "now")).to_be(4)
	app:click('role:button "Exnomic VPN"')
	app:frames(10)
	expect(app:state("filter")).to_be("orc-vpn")
	expect(count(app, "now")).to_be(1)
	expect(count(app, "review")).to_be(1)
end)

test("orch board: nothing on it is one quiet line, not empty columns", function()
	local app = board()
	app:set_state("orch_board_items", {})
	app:frames(10)
	expect(app:has(".orch-board-empty")).to_be(true)
	expect(app:has(".orch-column")).to_be(false)
	-- Something on it again: the columns, and no line.
	app:set_state("orch_board_items", {
		{ id = "it-a", orchestrator_id = "orc-orch", title = "One thing", state = "planned", priority = "normal" },
	})
	app:frames(10)
	expect(app:has(".orch-board-empty")).to_be(false)
	expect(count(app, "next")).to_be(1)
	for _, c in COLS do
		expect(app:has(`#orch-col-{c}`)).to_be(true)
	end
	-- A filter with nothing of its own: the line.
	app:set_state("filter", "orc-vpn")
	app:frames(10)
	expect(app:has(".orch-board-empty")).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch board: a card dragged to another column moves there", function()
	local app = board()
	app:drag('#orch-col-next >> role:button "Live end-to-end test on a dev engine"', '#orch-col-blocked >> role:button "Orchestrator screen"')
	app:frames(20)
	local calls = app:state("calls") :: string
	expect(calls).to_contain("move:it-live:blocked:")
	local order = tonumber(calls:match("move:it%-live:blocked:([%-%d%.]+)"))
	expect(order ~= nil).to_be(true)
	expect(count(app, "blocked")).to_be(3)
	expect(count(app, "next")).to_be(1)
	-- Nothing left lifted or moved aside.
	expect(app:has(".orch-column-drop")).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("orch board: a card dragged into an empty column lands there", function()
	local app = board()
	app:set_state("filter", "orc-orch")
	app:frames(10)
	expect(count(app, "review")).to_be(0)
	app:drag('#orch-col-next >> role:button "Live end-to-end test on a dev engine"', "#orch-col-review")
	app:frames(20)
	expect(app:state("calls")).to_contain("move:it-live:review:0")
	expect(count(app, "review")).to_be(1)
end)

test("orch board: a card dragged within its column takes an order between its neighbours", function()
	local app = board()
	app:set_state("filter", "orc-orch")
	app:frames(10)
	-- Reports (high, order 2) dropped onto Engine state (high, order 1).
	app:drag('#orch-col-now >> role:button "Reports, asks, MCP tools"', '#orch-col-now >> role:button "Engine: orchestration state + RPC"')
	app:frames(20)
	local calls = app:state("calls") :: string
	local order = tonumber(calls:match("move:it%-flow:active:([%-%d%.]+)"))
	expect(order ~= nil).to_be(true)
	-- Before Engine state (1): never where it was.
	expect(order ~= 2).to_be(true)
	expect(titles(app, "now")[2]).to_be("Reports, asks, MCP tools")
end)

test("orch board: a card stays among its priority", function()
	local app = board()
	-- Merge phase-1 (normal) dropped on Control room (urgent, first): it
	-- lands first of the normal ones, after the high ones.
	app:drag('#orch-col-now >> role:button "Merge phase-1 fixes"', '#orch-col-now >> role:button "Control room"')
	app:frames(20)
	local now = titles(app, "now")
	expect(now[1]).to_be("Control room")
	expect(now[4]).to_be("Merge phase-1 fixes")
end)

test("orch board: its menu sets a priority", function()
	local app = board()
	app:right_click('#orch-col-next >> role:button "Phase 2 migration"')
	app:frames(10)
	app:click('role:menuitem "Priority"')
	app:frames(10)
	app:click('role:menuitem "Urgent"')
	app:frames(20)
	expect(app:state("calls")).to_contain("priority:it-phase2:urgent")
	expect(titles(app, "next")[1]).to_be("Phase 2 migration")
end)

test("orch board: an outcome is two quiet lines", function()
	local app = board()
	expect(app:has("#orch-col-review >> .orch-outcome")).to_be(true)
	local _, _, _, h = box(app, "#orch-col-review >> .orch-outcome")
	-- (Two lines of the small size's, however long it is.)
	expect(h <= 34).to_be(true)
	expect(h >= 14).to_be(true)
	-- No button: pressed, it's the card's.
	expect(app:has('#orch-col-review >> role:button "Outcome"')).to_be(false)
end)

test("orch board: a card opens its thread, a PR chip its link", function()
	local app = board()
	app:click('#orch-col-now >> role:button "Control room"')
	app:frames(5)
	expect(app:state("calls")).to_contain("open:th-ui-control")
	app:click('#orch-col-done >> role:button "#1383"')
	app:frames(5)
	expect(app:state("calls")).to_contain("link:#1383")
end)

test("orch board: side by side wide, sideways narrower, stacked narrow, and back", function()
	local app = board(1400, 900)
	expect(app:has(".orch-board-side")).to_be(true)
	expect(app:has("#orch-board-scroll")).to_be(false)
	app:resize(800, 800)
	app:frames(20)
	expect(app:has("#orch-board-scroll")).to_be(true)
	local _, _, wn = box(app, "#orch-col-now")
	local _, _, wd = box(app, "#orch-col-done")
	expect(math.abs(wn - wd) < 1).to_be(true)
	expect(wn >= 184).to_be(true)
	app:resize(420, 800)
	app:frames(20)
	expect(app:has(".orch-board-stacked")).to_be(true)
	expect(count(app, "now")).to_be(4)
	app:set_state("compact", true)
	app:frames(20)
	expect(count(app, "now")).to_be(4)
	app:resize(1400, 900)
	app:frames(20)
	expect(app:has(".orch-board-side")).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch board: whose a card is, said only where the board holds more than one's", function()
	local app = board()
	-- All of them: each owned card names its orchestrator; the loose one none.
	expect(app:has('#orch-col-now >> role:button "Merge phase-1 fixes" >> .orch-card-owner >> "Exnomic VPN"')).to_be(true)
	expect(app:has('#orch-col-now >> role:button "Control room" >> .orch-card-owner >> "Orchestrators"')).to_be(true)
	expect(app:has("#orch-col-inbox >> .orch-card-owner")).to_be(false)
	-- One orchestrator's: nobody's named.
	app:set_state("filter", "orc-orch")
	app:frames(10)
	expect(app:has(".orch-card-owner")).to_be(false)
	-- Exnomic VPN's alone, the same.
	app:set_state("filter", "orc-vpn")
	app:frames(10)
	expect(count(app, "review")).to_be(1)
	expect(app:has(".orch-card-owner")).to_be(false)
end)

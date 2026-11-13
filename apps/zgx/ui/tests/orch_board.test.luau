-- The board (ds/orch_board, ui/orch_board_fixture.luau) on the sample:
-- its columns and what's in each, priority first, each column as wide as
-- the next; a card dragged to another column or within one says where (its
-- state and an order between its new neighbours); its menu sets a
-- priority; an outcome is two quiet lines; every card says whose it is
-- (but on its orchestrator's own page), its orchestrator's hue on its one
-- quiet line; an empty board is one line; and it
-- holds side by side when wide, in rows of three when narrower, stacked
-- when narrow. (Raw pointer drags, fast and slow, Escape
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

test("orch board: side by side wide, in rows narrower, stacked narrow, and back", function()
	local app = board(1400, 900)
	expect(app:has(".orch-board-side")).to_be(true)
	expect(app:has(".orch-board-rows")).to_be(false)
	-- Rows of three, every column as wide as the next, the last row's too.
	app:resize(800, 800)
	app:frames(20)
	expect(app:has(".orch-board-rows")).to_be(true)
	local _, yn, wn = box(app, "#orch-col-now")
	local _, yr, wr = box(app, "#orch-col-review")
	local _, yd, wd = box(app, "#orch-col-done")
	expect(math.abs(wn - wd) < 1).to_be(true)
	expect(math.abs(wn - wr) < 1).to_be(true)
	expect(yr > yn).to_be(true)
	expect(math.abs(yr - yd) < 1).to_be(true)
	expect(count(app, "done")).to_be(1)
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

test("orch board: a card carried between rows lands in the row below", function()
	local app = board(800, 900)
	expect(app:has(".orch-board-rows")).to_be(true)
	app:drag('#orch-col-now >> role:button "Merge phase-1 fixes"', "#orch-col-done")
	app:frames(20)
	expect(app:state("calls")).to_contain("move:it-phase1:done:")
	expect(count(app, "done")).to_be(2)
end)

test("orch board: every card says whose it is, its orchestrator's hue named", function()
	local app = board()
	-- Its orchestrator's hue on its quiet line, named for the pointer and
	-- for a screen reader; no row of names and folders under it.
	expect(app:has('#orch-col-now >> role:button "Merge phase-1 fixes" >> .orch-card-owner')).to_be(true)
	expect(app:has('#orch-col-now >> role:button "Merge phase-1 fixes" >> role:img "Exnomic VPN"')).to_be(true)
	expect(app:has('#orch-col-now >> role:button "Control room" >> role:img "Orchestrators"')).to_be(true)
	expect(app:has(".orch-card-orchestrator")).to_be(false)
	expect(app:has(".orch-card-project")).to_be(false)
	-- Filtered to one orchestrator (the Control room's chip), still said.
	app:set_state("filter", "orc-orch")
	app:frames(10)
	expect(app:has('#orch-col-now >> role:button "Control room" >> role:img "Orchestrators"')).to_be(true)
	-- Loose items: no one's.
	app:set_state("filter", nil)
	app:set_state("orch_board_items", {
		{ id = "it-a", orchestrator_id = "orc-vpn", title = "One thing", state = "planned", priority = "normal" },
		{ id = "it-b", title = "Loose with a thread", state = "planned", priority = "normal", threads = { "th-loose" } },
		{ id = "it-c", title = "Loose alone", state = "planned", priority = "normal" },
	})
	app:frames(10)
	expect(app:has('#orch-col-next >> role:button "One thing" >> role:img "Exnomic VPN"')).to_be(true)
	expect(app:has('#orch-col-next >> role:button "Loose with a thread" >> .orch-card-owner')).to_be(false)
	expect(app:has('#orch-col-next >> role:button "Loose alone" >> .orch-card-owner')).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("orch board: in the Control room, a done card says whose", function()
	local app = mount("control_board_live_fixture.luau")
	app:resize(1440, 2600)
	app:frames(30)
	expect(app:has('#control-board-done >> role:img "Tally"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch board: on an orchestrator's own page, no card says whose", function()
	local app = mount("orchestrator_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	expect(app:has('#orch-page-board >> "Control room"')).to_be(true)
	expect(app:has("#orch-page-board >> .orch-card-owner")).to_be(false)
end)

test("orch board: two orchestrators of one name, their chips say their projects", function()
	local app = board()
	expect(app:has(".orch-filter-project")).to_be(false)
	local list = table.clone(app:state("orch_board_orchestrators") :: { any })
	for i, o in list do
		if o.id == "orc-ci" then
			local twin = table.clone(o)
			twin.name = "Orchestrators"
			list[i] = twin
		end
	end
	app:set_state("orch_board_orchestrators", list)
	app:frames(10)
	expect(app:has('.orch-board-filter >> .orch-filter-project >> "zeron"')).to_be(true)
end)

test("orch board: a card made from a dump's part shows that dump from its menu", function()
	local app = board()
	app:set_state("orch_board_items", {
		{ id = "it-d", orchestrator_id = "orc-orch", title = "From what was said", state = "planned", priority = "normal", source = { dump_id = "dp-2", route_id = "rt-5" } },
		{ id = "it-e", orchestrator_id = "orc-orch", title = "Made by hand", state = "planned", priority = "normal" },
	})
	app:frames(10)
	-- No mark of it on the card: its menu says it.
	expect(app:has(".orch-card-source")).to_be(false)
	app:right_click('#orch-col-next >> role:button "Made by hand"')
	app:frames(10)
	expect(app:has('role:menuitem "Show dump"')).to_be(false)
	app:key("Escape")
	app:frames(10)
	app:right_click('#orch-col-next >> role:button "From what was said"')
	app:frames(10)
	app:click('role:menuitem "Show dump"')
	app:frames(10)
	expect(app:state("calls")).to_contain("source:dp-2")
	expect(app:state("calls")).never.to_contain("open:")
end)

test("orch board: in the Control room, a card's dump is brought into view", function()
	local app = mount("control_page_fixture.luau")
	-- (Tall enough for the Board under both columns, Q&A's in the right.)
	app:resize(1440, 3000)
	app:frames(30)
	-- The loose item the newest dump made, in the Inbox: its menu's Show dump.
	app:right_click("#control-board-inbox >> .orch-card")
	app:frames(10)
	app:click('role:menuitem "Show dump"')
	app:frames(10)
	expect(app:error()).to_be_nil()
end)

test("orch board: done today is since midnight, on the board and in the Control room's count alike", function()
	local app = mount("control_board_live_fixture.luau")
	app:resize(1440, 2600)
	app:frames(30)
	local now = os.time()
	local here = os.date("*t", now) :: any
	local midnight = now - (here.hour * 3600 + here.min * 60 + here.sec)
	local n = table.clone(app:state("orchestration") :: any)
	n.items = {
		{ id = "it-today", orchestrator_id = "orc-tally", title = "Done this morning", state = "done", priority = "normal", order = 1, done = midnight, updated = midnight },
		-- (Within the last day, but before midnight: yesterday's.)
		{ id = "it-yesterday", orchestrator_id = "orc-tally", title = "Done last night", state = "done", priority = "normal", order = 2, done = midnight - 60, updated = midnight - 60 },
	}
	app:set_state("orchestration", n)
	app:frames(20)
	expect(app:has('#control-board-done >> role:button "Done this morning"')).to_be(true)
	expect(app:has('#control-board-done >> role:button "Done last night"')).to_be(false)
	expect(app:has('#control-widget-board >> "1 done today"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch board: a card's menu, its thread, Move to, priority, its dump, Delete", function()
	local app = board()
	app:set_state("orch_board_items", {
		{ id = "it-d", orchestrator_id = "orc-orch", title = "From what was said", state = "planned", priority = "normal", order = 1, threads = { "th-ui-control" }, source = { dump_id = "dp-2", route_id = "rt-5" } },
	})
	app:frames(10)
	app:right_click('#orch-col-next >> role:button "From what was said"')
	app:frames(10)
	for _, name in { "Open thread", "Move to", "Priority", "Show dump", "Delete" } do
		expect(app:has(`role:menuitem "{name}"`)).to_be(true)
	end
	-- Show dump.
	app:click('role:menuitem "Show dump"')
	app:frames(20)
	expect(app:state("calls")).to_contain("source:dp-2")
	-- Move to.
	app:right_click('#orch-col-next >> role:button "From what was said"')
	app:frames(10)
	app:click('role:menuitem "Move to"')
	app:frames(10)
	app:click('role:menuitem "Review"')
	app:frames(20)
	expect(app:state("calls")).to_contain("move:it-d:review:")
	expect(app:has('#orch-col-review >> role:button "From what was said"')).to_be(true)
end)

test("orch board: Delete takes a card off the board at once, Undo puts it back", function()
	local app = board()
	app:right_click('#orch-col-next >> role:button "Phase 2 migration"')
	app:frames(10)
	app:click('role:menuitem "Delete"')
	app:frames(20)
	expect(app:state("calls")).to_contain("remove:it-phase2")
	expect(app:has('#orch-col-next >> role:button "Phase 2 migration"')).to_be(false)
	-- Never its chat: no other call.
	expect(app:state("calls")).never.to_contain("open:")
	expect(app:has('"Deleted from the board"')).to_be(true)
	app:click('role:button "Undo"')
	app:frames(20)
	expect(app:state("calls")).to_contain("restore:it-phase2:Phase 2 migration")
	expect(app:has('#orch-col-next >> role:button "Phase 2 migration"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

-- The cards themselves (ui/orch_board_cards_fixture.luau): a title and one
-- quiet line, the rest only where it says something.
local function cards(): any
	local app = mount("orch_board_cards_fixture.luau")
	app:resize(1440, 900)
	app:frames(20)
	return app
end
local LEGACY = "Feature request for the Board: the person wants to delete a card from its menu, with an Undo beside it for a while"

test("orch board cards: a long title is two lines at most", function()
	local app = cards()
	local _, _, _, h = box(app, `#cards-now >> role:button "{LEGACY}" >> .orch-card-title`)
	expect(h > 14).to_be(true)
	expect(h <= 34).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch board cards: priority is a word on the quiet line, urgent and high alone", function()
	local app = cards()
	expect(app:has('#cards-now >> role:button "Fix slash menu in Codex chats" >> .orch-card-priority >> "Urgent"')).to_be(true)
	expect(app:has('#cards-now >> role:button "Scroll position resets on live re-renders" >> .orch-card-priority >> "High"')).to_be(true)
	expect(app:has('#cards-now >> role:button "Delete cards from the Board" >> .orch-card-priority')).to_be(false)
	expect(app:has('#cards-next >> role:button "Phase 2 migration" >> .orch-card-priority')).to_be(false)
end)

test("orch board cards: progress only once a step is done, one clock", function()
	local app = cards()
	local scroll = '#cards-now >> role:button "Scroll position resets on live re-renders"'
	expect(app:has(`{scroll} >> .orch-card-progress >> "3/7"`)).to_be(true)
	-- 25 minutes at it.
	expect(app:has(`{scroll} >> .orch-card-clock >> "25m"`)).to_be(true)
	-- Nothing of its five steps done yet: no bar; stalled 15 minutes.
	local slash = '#cards-now >> role:button "Fix slash menu in Codex chats"'
	expect(app:has(`{slash} >> .orch-card-progress`)).to_be(false)
	expect(app:has(`{slash} >> .orch-card-word >> "Stalled"`)).to_be(true)
	expect(app:has(`{slash} >> .orch-card-clock >> "15m"`)).to_be(true)
	-- No thread, no todo: whose it is alone.
	expect(app:has('#cards-now >> role:button "Delete cards from the Board" >> .orch-card-clock')).to_be(false)
	expect(app:has('#cards-now >> role:button "Delete cards from the Board" >> .orch-card-owner-name >> "Zeron fixes"')).to_be(true)
	expect(app:has('#cards-now >> role:button "Fix slash menu in Codex chats" >> .orch-card-owner-name')).to_be(false)
	-- Its own share, once there's some.
	expect(app:has('#cards-next >> role:button "Write the mesh runbook" >> .orch-card-progress >> "40%"')).to_be(true)
	-- Waiting on the person: how long since it moved.
	expect(app:has('#cards-blocked >> role:button "Approve engine migration" >> .orch-card-word >> "Needs you"')).to_be(true)
	expect(app:has('#cards-blocked >> role:button "Approve engine migration" >> .orch-card-clock >> "10m"')).to_be(true)
	expect(app:has('#cards-blocked >> role:button "DNS failover between regions" >> .orch-card-word >> "Failed"')).to_be(true)
	-- No old strip of its own clocks and menu.
	expect(app:has(".orch-progress")).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("orch board cards: its menu shows where the pointer rests, and deletes", function()
	local app = cards()
	local card = '#cards-now >> role:button "Delete cards from the Board"'
	expect(app:has(`{card} >> .orch-card-menu`)).to_be(false)
	app:hover(card)
	app:frames(10)
	expect(app:has(`{card} >> .orch-card-menu`)).to_be(true)
	app:click(`{card} >> .orch-card-menu`)
	app:frames(10)
	-- Its own press, not the card's: no thread opened, its menu open.
	expect(app:state("calls")).never.to_contain("open:")
	expect(app:has('role:menuitem "Move to"')).to_be(true)
	app:click('role:menuitem "Delete"')
	app:frames(20)
	expect(app:state("calls")).to_contain("remove:it-delete")
	expect(app:has(card)).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("orch board cards: right-click Delete still takes a card off", function()
	local app = cards()
	app:right_click(`#cards-now >> role:button "{LEGACY}"`)
	app:frames(10)
	app:click('role:menuitem "Delete"')
	app:frames(20)
	expect(app:state("calls")).to_contain("remove:it-legacy")
	expect(app:has(`#cards-now >> role:button "{LEGACY}"`)).to_be(false)
	expect(app:has('"Deleted from the board"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch board cards: a click opens its thread", function()
	local app = cards()
	app:click('#cards-now >> role:button "Scroll position resets on live re-renders"')
	app:frames(5)
	expect(app:state("calls")).to_contain("open:th-scroll")
end)

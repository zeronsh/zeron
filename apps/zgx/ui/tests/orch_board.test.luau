-- The board (ds/orch_board) on ui/orch_board_places_fixture.luau: its
-- tabs and their counts, the orchestrators' filter, the board/list switch,
-- the states a host keeps (view, tab, selected, drawers), whose each card
-- is, a card's lines, a paused card's Resume, Done since midnight, and the
-- columns side by side, stacked when narrow or compact, a card's detail
-- beside them. On the Control room's Board (ui/control_page_fixture.luau,
-- the sample's rows): whose a done card is, and a card's dump brought into
-- view. (Columns, drawers and their order: tests/orch_board_columns; the
-- model: tests/orch_board_model.)

local base = os.time()
local here = os.date("*t", base) :: any
local MIDNIGHT = base - (here.hour * 3600 + here.min * 60 + here.sec)
local T = MIDNIGHT + 20 * 3600
local HOUR = 3600

local function board(w: number?, h: number?): any
	local app = mount("orch_board_places_fixture.luau")
	app:resize(w or 1500, h or 1200)
	app:frames(30)
	return app
end

-- Whether a node shows (there, and not hidden).
local function shown(app: any, sel: string): boolean
	if not app:has(sel) then
		return false
	end
	local first = string.match(app:tree(sel, 0), "[^\n]+") or ""
	return string.find(first, " hidden", 1, true) == nil
end

-- A node's box as the tree says it.
local function box(app: any, sel: string): { x: number, y: number, w: number, h: number }
	local first = string.match(app:tree(sel, 0), "[^\n]+") or ""
	local x, y, w, h = string.match(first, " (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
	return { x = tonumber(x) or 0, y = tonumber(y) or 0, w = tonumber(w) or 0, h = tonumber(h) or 0 }
end

local function calls(app: any): string
	return app:state("calls") :: string
end

local function tab_count(app: any, label: string): string
	local line = string.match(app:tree(`#orch-board-tabs >> role:tab "{label}"`, 1), "\n%s+@%d+ text \"(%d+)\"")
	return line or ""
end

local function add(app: any, rows: { { item: any, card: any? } })
	local items = table.clone(app:state("fx_items") :: { any })
	local cards = table.clone(app:state("fx_cards") :: { any })
	for _, r in rows do
		table.insert(items, r.item)
		if r.card then
			local c = table.clone(r.card)
			c.id = r.item.id
			c.old_state = c.old_state or r.item.state
			c.asks = c.asks or {}
			table.insert(cards, c)
		end
	end
	app:set_state("fx_items", items)
	app:set_state("fx_cards", cards)
	app:frames(20)
end

local COLUMNS = { "needs_you", "running", "done" }

test("orch board: the tabs count today's cards, the three columns' and not their drawers'", function()
	local app = board()
	-- 3 + 5 + 3; Snoozed's one, Planned's four and Earlier's five aside.
	expect(tab_count(app, "All tasks")).to_be("11")
	expect(tab_count(app, "Needs you")).to_be("3")
	expect(tab_count(app, "Running")).to_be("5")
	expect(tab_count(app, "Done")).to_be("3")
	expect(app:error()).to_be_nil()
end)

test("orch board: a tab shows its column alone, its drawer with it", function()
	local app = board()
	for _, c in COLUMNS do
		expect(shown(app, `#orch-col-{c}`)).to_be(true)
	end
	app:click('#orch-board-tabs >> role:tab "Needs you"')
	app:frames(20)
	expect(app:state("board_tab")).to_be("needs_you")
	expect(shown(app, "#orch-col-needs_you")).to_be(true)
	expect(shown(app, "#orch-col-running")).to_be(false)
	expect(shown(app, "#orch-col-done")).to_be(false)
	expect(shown(app, "#orch-col-needs_you >> #orch-drawer-snoozed")).to_be(true)
	app:click('#orch-board-tabs >> role:tab "Done"')
	app:frames(20)
	expect(shown(app, "#orch-col-done")).to_be(true)
	expect(shown(app, "#orch-col-needs_you")).to_be(false)
	app:click('#orch-board-tabs >> role:tab "All tasks"')
	app:frames(20)
	for _, c in COLUMNS do
		expect(shown(app, `#orch-col-{c}`)).to_be(true)
	end
	expect(app:error()).to_be_nil()
end)

test("orch board: in the list, a tab shows its sections; Running's shows Planned too", function()
	local app = board()
	app:set_state("board_view", "list")
	app:frames(20)
	-- (A section hidden by its tab: its fold, inside it, hidden with it.)
	local function section_shown(s: string): boolean
		local t = app:tree("#orch-list", 2)
		for line in t:gmatch("[^\n]+") do
			if string.find(line, `#orch-list-{s} `, 1, true) then
				return string.find(line, " hidden", 1, true) == nil
			end
		end
		return false
	end
	for _, s in { "needs_you", "running", "planned", "done" } do
		expect(section_shown(s)).to_be(true)
	end
	app:click('#orch-board-tabs >> role:tab "Running"')
	app:frames(20)
	expect(section_shown("running")).to_be(true)
	expect(section_shown("planned")).to_be(true)
	expect(section_shown("needs_you")).to_be(false)
	expect(section_shown("done")).to_be(false)
	app:click('#orch-board-tabs >> role:tab "Done"')
	app:frames(20)
	expect(section_shown("done")).to_be(true)
	expect(section_shown("running")).to_be(false)
	expect(section_shown("planned")).to_be(false)
	-- (Earlier folds under Done, there with it.)
	expect(app:has("#orch-list-done >> #orch-list-earlier")).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch board: the filter's menu names every orchestrator; one chosen, its cards alone", function()
	local app = board()
	app:click("#orch-board-filter")
	app:frames(10)
	expect(app:has('role:menuitemcheckbox "All orchestrators"')).to_be(true)
	for _, name in { "Zeron fixes", "Exnomic VPN", "CI speed" } do
		expect(app:has(`role:menuitemcheckbox "{name}"`)).to_be(true)
	end
	app:click('role:menuitemcheckbox "Exnomic VPN"')
	app:frames(20)
	expect(app:state("filter")).to_be("orc-vpn")
	-- The button says whose; the tabs and columns count its cards alone.
	expect(app:has('#orch-board-filter >> "Exnomic VPN"')).to_be(true)
	expect(tab_count(app, "All tasks")).to_be("3")
	expect(app:text("#orch-col-needs_you >> .orch-column-count")).to_be("2")
	expect(app:text("#orch-col-running >> .orch-column-count")).to_be("1")
	expect(app:has("#orch-col-done >> .orch-column-empty")).to_be(true)
	expect(app:has("#orch-card-it-merge")).to_be(false)
	expect(app:has("#orch-card-it-admin")).to_be(true)
	-- All of them again.
	app:click("#orch-board-filter")
	app:frames(10)
	app:click('role:menuitemcheckbox "All orchestrators"')
	app:frames(20)
	expect(app:state("filter")).to_be_nil()
	expect(tab_count(app, "All tasks")).to_be("11")
	expect(app:has('#orch-board-filter >> "Orchestrators"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch board: the board and its list, switched in its head", function()
	local app = board()
	expect(app:has("#orch-columns")).to_be(true)
	expect(app:has("#orch-list")).to_be(false)
	app:click("#orch-board-view-list")
	app:frames(20)
	expect(app:state("board_view")).to_be("list")
	expect(app:has("#orch-columns")).to_be(false)
	-- (Needs you's 3, Running's 5, Planned's 4, Done's 3; Snoozed and
	-- Earlier folded.)
	expect(app:count("#orch-list >> .orch-list-row")).to_be(15)
	app:click("#orch-board-view-board")
	app:frames(20)
	expect(app:state("board_view")).to_be("board")
	expect(app:has("#orch-list")).to_be(false)
	expect(app:count("#orch-columns >> .orch-card")).to_be(11)
	expect(app:error()).to_be_nil()
end)

test("orch board: the states a host keeps: its view, tab, open card and drawers", function()
	local app = board()
	-- Set by the host: the board follows.
	app:set_state("board_drawers", { planned = true })
	app:frames(20)
	expect(app:count("#orch-drawer-planned >> .orch-card")).to_be(4)
	expect(app:count("#orch-drawer-earlier >> .orch-card")).to_be(0)
	app:set_state("board_selected", "it-token")
	app:frames(20)
	expect(app:text("#orch-detail-title")).to_be("Token usage breakdown")
	app:set_state("board_tab", "running")
	app:frames(20)
	expect(shown(app, "#orch-col-running")).to_be(true)
	expect(shown(app, "#orch-col-done")).to_be(false)
	app:set_state("board_view", "list")
	app:frames(20)
	expect(app:has("#orch-list")).to_be(true)
	-- Changed by the person: the host's states change.
	app:set_state("board_view", "board")
	app:set_state("board_tab", "all")
	app:set_state("board_selected", nil)
	app:frames(20)
	expect(app:has("#orch-board-detail")).to_be(false)
	app:click("#orch-drawer-earlier >> .orch-drawer-head")
	app:frames(20)
	local drawers = app:state("board_drawers") :: any
	expect(drawers.earlier).to_be(true)
	expect(drawers.planned).to_be(true)
	app:click("#orch-card-it-chat")
	app:frames(20)
	expect(app:state("board_selected")).to_be("it-chat")
	app:click('#orch-board-tabs >> role:tab "Needs you"')
	app:frames(20)
	expect(app:state("board_tab")).to_be("needs_you")
	expect(app:error()).to_be_nil()
end)

test("orch board: a card opens its detail beside the board; Close shuts it; its thread opens from it", function()
	local app = board()
	app:click("#orch-card-it-chat")
	app:frames(20)
	expect(app:has("#orch-board-detail")).to_be(true)
	expect(app:text("#orch-detail-title")).to_be("Chat list: only changed rows update")
	-- Beside the columns, at their right; the columns narrower for it.
	local d, cols = box(app, "#orch-board-detail"), box(app, "#orch-columns")
	expect(d.x >= cols.x + cols.w).to_be(true)
	expect(math.abs(d.y - cols.y) < 1).to_be(true)
	-- (A click opens the card, never its thread.)
	expect(calls(app)).never.to_contain("open:")
	app:click("#orch-detail-open")
	app:frames(10)
	expect(calls(app)).to_contain("open:th-chat")
	app:click("#orch-detail-close")
	app:frames(20)
	expect(app:has("#orch-board-detail")).to_be(false)
	expect(app:state("board_selected")).to_be_nil()
	expect(app:error()).to_be_nil()
end)

test("orch board: side by side wide, every column as wide as the next; stacked narrow, each the board's width; and back", function()
	local app = board(1500, 1400)
	local function boxes(): { any }
		local out = {}
		for _, c in COLUMNS do
			table.insert(out, box(app, `#orch-col-{c}`))
		end
		return out
	end
	local b = boxes()
	for i = 2, 3 do
		expect(math.abs(b[i].w - b[1].w) < 1).to_be(true)
		expect(math.abs(b[i].y - b[1].y) < 1).to_be(true)
		expect(b[i].x > b[1].x).to_be(true)
	end
	-- (Heads level, on one line each.)
	expect(box(app, "#orch-col-running >> .orch-column-head").h <= 30).to_be(true)
	app:resize(700, 2400)
	app:frames(30)
	local lanes = box(app, "#orch-columns")
	b = boxes()
	for i = 1, 3 do
		expect(math.abs(b[i].w - lanes.w) < 1).to_be(true)
		expect(math.abs(b[i].x - lanes.x) < 1).to_be(true)
		if i > 1 then
			expect(b[i].y > b[i - 1].y).to_be(true)
		end
	end
	expect(app:count("#orch-col-running >> .orch-card")).to_be(5)
	app:resize(1500, 1400)
	app:frames(30)
	b = boxes()
	expect(math.abs(b[2].y - b[1].y) < 1).to_be(true)
	-- Compact: stacked however wide.
	app:set_state("compact", true)
	app:frames(30)
	b = boxes()
	expect(b[2].y > b[1].y).to_be(true)
	expect(math.abs(b[1].w - box(app, "#orch-columns").w) < 1).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch board: compact, a card's detail takes the board's width", function()
	local app = board(1000, 1400)
	app:set_state("compact", true)
	app:frames(30)
	app:set_state("board_selected", "it-merge")
	app:frames(30)
	local d, b = box(app, "#orch-board-detail"), box(app, "#orch-board")
	expect(math.abs(d.w - b.w) < 1).to_be(true)
	expect(app:text("#orch-detail-title")).to_be("Bring main (0.3.8) into orchestrators")
	expect(app:has('#orch-detail-needs >> role:button "Unlocked + push the fork"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch board: every card says whose it is in the Control room; none in an orchestrator's own list", function()
	local app = board()
	expect(app:count("#orch-columns >> .orch-hue-dot")).to_be(app:count("#orch-columns >> .orch-card"))
	app:set_state("board_view", "list")
	app:frames(20)
	expect(app:count("#orch-list >> .orch-hue-dot")).to_be(app:count("#orch-list >> .orch-list-row"))
	-- The list by itself, one orchestrator's (its page's panel): no one's.
	app:set_state("only_list", true)
	app:set_state("compact", true)
	app:frames(30)
	expect(app:count("#fixture-list >> .orch-list-row") > 0).to_be(true)
	expect(app:has("#fixture-list >> .orch-hue-dot")).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("orch board: in the Control room, a done card says whose", function()
	local app = mount("control_board_live_fixture.luau")
	app:resize(1440, 2600)
	app:frames(30)
	expect(app:count("#orch-col-done >> .orch-card") > 0).to_be(true)
	expect(app:count("#orch-col-done >> .orch-hue-dot")).to_be(app:count("#orch-col-done >> .orch-card"))
	expect(app:error()).to_be_nil()
end)

test("orch board: on an orchestrator's own page, no card says whose", function()
	local app = mount("orchestrator_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	expect(app:has('#orch-page-board >> "Control room"')).to_be(true)
	expect(app:has("#orch-page-board >> .orch-hue-dot")).to_be(false)
end)

test("orch board: a title is two lines at most, a summary two quiet lines; a drawer's card none", function()
	local app = board()
	add(app, {
		{
			item = { id = "it-long", orchestrator_id = "orc-ci", state = "active", priority = "normal", order = 1, threads = {}, created = T - HOUR, updated = T - HOUR, links = {},
				title = "Feature request for the Board: the person wants to delete a card from its menu, with an Undo beside it for a while, and more words still to fill three lines",
				detail = string.rep("A long description that goes on and on. ", 12) },
			card = { place = "running", status = "working", since = T - 10, status_since = T - 10, seq = 1, events = {} },
		},
	})
	local title = box(app, "#orch-card-it-long >> .orch-card-title")
	expect(title.h > 17).to_be(true)
	expect(title.h <= 34).to_be(true)
	local summary = box(app, "#orch-card-it-long >> .orch-card-summary")
	expect(summary.h > 17).to_be(true)
	expect(summary.h <= 34).to_be(true)
	-- In a drawer: its title and foot, no summary (Keyed sidebar shape has
	-- a detail to say).
	app:click("#orch-drawer-planned >> .orch-drawer-head")
	app:frames(20)
	expect(app:has("#orch-card-it-keyed")).to_be(true)
	expect(app:has("#orch-card-it-keyed >> .orch-card-summary")).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("orch board: a queued card waits for a slot; started, it says Starting", function()
	local app = board()
	add(app, {
		{ item = { id = "it-q", orchestrator_id = "orc-ci", title = "Queued one", state = "planned", waiting = "Ops's crew is full", priority = "normal", order = 0, threads = {}, created = T - HOUR, updated = T - HOUR, links = {} } },
		{
			item = { id = "it-s", orchestrator_id = "orc-ci", title = "Started one", state = "active", priority = "normal", order = 1, threads = {}, created = T - HOUR, updated = T - 5, links = {} },
			card = { place = "running", status = "queued", since = T - 5, status_since = T - 5, seq = 1, events = {} },
		},
	})
	app:click("#orch-drawer-planned >> .orch-drawer-head")
	app:frames(20)
	expect(app:text("#orch-card-it-q >> .orch-card-status")).to_be("Waiting for a slot")
	expect(app:text("#orch-card-it-s >> .orch-card-status")).to_be("Starting")
	expect(app:error()).to_be_nil()
end)

test("orch board: a paused card has a Resume of its own; it resumes into Running", function()
	local app = board()
	app:click("#orch-drawer-planned >> .orch-drawer-head")
	app:frames(20)
	expect(app:has("#orch-card-it-exits >> .orch-card-resume")).to_be(true)
	-- Nobody else's: not Planned's others, not Needs you's.
	expect(app:count(".orch-card-resume")).to_be(1)
	app:click("#orch-card-it-exits >> .orch-card-resume")
	app:frames(20)
	expect(calls(app)).to_contain("resume:it-exits")
	-- (Its own press: the card isn't opened.)
	expect(app:state("board_selected")).to_be_nil()
	expect(app:has("#orch-col-running >> #orch-card-it-exits")).to_be(true)
	expect(app:has("#orch-card-it-exits >> .orch-card-resume")).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("orch board: Done is what closed since midnight; before it, Earlier", function()
	local app = board()
	local function closed(id: string, at: number): any
		return {
			item = { id = id, orchestrator_id = "orc-ci", title = id, state = "done", priority = "normal", order = 1, threads = {}, created = at - HOUR, updated = at, done = at, links = {} },
			card = { place = "done", status = "done", since = at, status_since = at, seq = 2, closed = { at = at, by = "engine", how = "report" }, events = {} },
		}
	end
	add(app, { closed("it-morning", MIDNIGHT), closed("it-last-night", MIDNIGHT - 60) })
	expect(app:has("#orch-col-done >> #orch-card-it-morning")).to_be(true)
	expect(tab_count(app, "Done")).to_be("4")
	expect(app:text("#orch-drawer-earlier >> .orch-drawer-count")).to_be("6")
	app:click("#orch-drawer-earlier >> .orch-drawer-head")
	app:frames(20)
	expect(app:has("#orch-drawer-earlier >> #orch-card-it-last-night")).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch board: in the Control room, a card's dump is brought into view", function()
	local app = mount("control_page_fixture.luau")
	app:resize(1440, 3000)
	app:frames(30)
	app:set_state("control_tab", "board")
	app:frames(30)
	-- The loose card the newest dump made, held: in Planned.
	app:click("#control-board >> #orch-drawer-planned >> .orch-drawer-head")
	app:frames(20)
	app:right_click('#control-board >> role:button "Rerun omarchy-update (needs sudo)"')
	app:frames(10)
	app:click('role:menuitem "Show dump"')
	app:frames(30)
	-- (In Capture's history, opened over the page.)
	expect(app:state("control_history_panel")).to_be(true)
	expect(app:error()).to_be_nil()
end)

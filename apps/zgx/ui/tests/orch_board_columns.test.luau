-- The board's columns and drawers (ds/orch_board, on
-- ui/orch_board_places_fixture.luau and rows added to it): each column's
-- and drawer's count, what a folded drawer says, the order within each
-- place, twelve cards a column and twenty a drawer before "Show n more",
-- the empty lines, and where a card with no row (or a row an older engine
-- has overtaken) shows.

-- The fixture's "now": today at 8 PM here.
local base = os.time()
local here = os.date("*t", base) :: any
local T = base - (here.hour * 3600 + here.min * 60 + here.sec) + 20 * 3600
local MIN, HOUR = 60, 3600

local function board(w: number?, h: number?): any
	local app = mount("orch_board_places_fixture.luau")
	app:resize(w or 1500, h or 1200)
	app:frames(30)
	return app
end

-- The cards' ids in a column (not its drawer's) or in a drawer, in order.
local function column(app: any, place: string): { string }
	local out: { string } = {}
	for line in app:tree(`#orch-col-{place}`):gmatch("[^\n]+") do
		local id = line:match("^  @%d+ #orch%-card%-(%S+)")
		if id then
			table.insert(out, id)
		end
	end
	return out
end
local function drawer(app: any, place: string): { string }
	local out: { string } = {}
	for line in app:tree(`#orch-drawer-{place}`):gmatch("[^\n]+") do
		local id = line:match("#orch%-card%-(%S+)")
		if id then
			table.insert(out, id)
		end
	end
	return out
end
local function joined(list: { string }): string
	return table.concat(list, ",")
end

-- Cards added to the fixture's rows: `{ item, card? }` (no card: no row).
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
local function patch(app: any, key: string, id: string, fields: { [string]: any })
	local rows = table.clone(app:state(key) :: { any })
	for i, r in rows do
		if r.id == id then
			local copy = table.clone(r)
			for k, v in fields do
				copy[k] = v
			end
			rows[i] = copy
		end
	end
	app:set_state(key, rows)
	app:frames(20)
end

local function many(n: number, place: string, status: string, state: string, prefix: string): { { item: any, card: any } }
	local out = {}
	for i = 1, n do
		local at = T - (5 * HOUR + i * MIN)
		table.insert(out, {
			item = { id = `{prefix}-{i}`, orchestrator_id = "orc-ci", title = `{prefix} {i}`, state = state, priority = "normal", order = 100 + i, threads = {}, created = at, updated = at, links = {} },
			card = { place = place, status = status, since = at, status_since = at, seq = 1, events = {} },
		})
	end
	return out
end

local function one(id: string, place: string, status: string, state: string, extra: { [string]: any }?): { item: any, card: any }
	local card: { [string]: any } = { place = place, status = status, since = T - HOUR, status_since = T - HOUR, seq = 1, events = {} }
	for k, v in extra or {} do
		card[k] = v
	end
	return {
		item = { id = id, orchestrator_id = "orc-ci", title = id, state = state, priority = "normal", order = 1, threads = {}, created = T - HOUR, updated = T - HOUR, links = {} },
		card = card,
	}
end

test("board columns: each column's head counts its cards, each drawer's head its own", function()
	local app = board()
	expect(app:text("#orch-col-needs_you >> .orch-column-count")).to_be("3")
	expect(app:text("#orch-col-running >> .orch-column-count")).to_be("5")
	expect(app:text("#orch-col-done >> .orch-column-count")).to_be("3")
	expect(#column(app, "needs_you")).to_be(3)
	expect(#column(app, "running")).to_be(5)
	expect(#column(app, "done")).to_be(3)
	-- Folded, a drawer says how many and what's first; none of its cards.
	expect(app:text("#orch-drawer-snoozed >> .orch-drawer-count")).to_be("1")
	expect(app:text("#orch-drawer-planned >> .orch-drawer-count")).to_be("4")
	expect(app:text("#orch-drawer-earlier >> .orch-drawer-count")).to_be("5")
	expect(app:text("#orch-drawer-snoozed >> .orch-drawer-hint")).to_be("12 pull requests to review")
	expect(app:has("#orch-drawer-snoozed >> .orch-drawer-back")).to_be(true)
	expect(app:text("#orch-drawer-planned >> .orch-drawer-hint")).to_be("Next: Keyed sidebar shape")
	expect(app:has("#orch-drawer-planned >> .orch-drawer-back")).to_be(false)
	expect(app:text("#orch-drawer-earlier >> .orch-drawer-hint")).to_be("before today")
	for _, d in { "snoozed", "planned", "earlier" } do
		expect(#drawer(app, d)).to_be(0)
	end
	-- Opened: its cards, its hint gone; the column's count still its own.
	app:click("#orch-drawer-earlier >> .orch-drawer-head")
	app:frames(20)
	expect(#drawer(app, "earlier")).to_be(5)
	expect(app:has("#orch-drawer-earlier >> .orch-drawer-hint")).to_be(false)
	expect(app:text("#orch-col-done >> .orch-column-count")).to_be("3")
	expect(#column(app, "done")).to_be(3)
	-- Folded again.
	app:click("#orch-drawer-earlier >> .orch-drawer-head")
	app:frames(20)
	expect(#drawer(app, "earlier")).to_be(0)
	expect(app:error()).to_be_nil()
end)

test("board columns: the order within each column and drawer", function()
	local app = board()
	app:set_state("board_drawers", { snoozed = true, planned = true, earlier = true })
	app:frames(20)
	-- Needs you: the one that came last first; Running too.
	expect(joined(column(app, "needs_you"))).to_be("it-merge,it-admin,it-relay")
	expect(joined(column(app, "running"))).to_be("it-chat,it-release,it-tool,it-telemetry,it-ios")
	-- Done and Earlier: closed last first.
	expect(joined(column(app, "done"))).to_be("it-asks,it-token,it-msi")
	expect(joined(drawer(app, "earlier"))).to_be("it-old-1,it-old-2,it-old-3,it-old-4,it-old-5")
	-- Planned: what starts next, then what waits on another, paused, held.
	expect(joined(drawer(app, "planned"))).to_be("it-keyed,it-win,it-exits,it-probes")
	expect(joined(drawer(app, "snoozed"))).to_be("it-prs")
	-- An urgent ask goes first in Needs you, however old.
	patch(app, "fx_asks", "ak-stall", { urgent = true })
	expect(joined(column(app, "needs_you"))).to_be("it-relay,it-merge,it-admin")
	-- Snoozed: the one back first.
	add(app, { one("it-snz", "snoozed", "asked", "blocked", { ["until"] = T + 30 * MIN }) })
	expect(joined(drawer(app, "snoozed"))).to_be("it-snz,it-prs")
	expect(app:error()).to_be_nil()
end)

test("board columns: twelve a column, then Show n more", function()
	local app = board(1500, 2600)
	add(app, many(15, "running", "working", "active", "run"))
	expect(app:text("#orch-col-running >> .orch-column-count")).to_be("20")
	expect(#column(app, "running")).to_be(12)
	expect(app:text("#orch-col-running >> .orch-show-more")).to_be("Show 8 more")
	-- (Only the column past its cap says so.)
	expect(app:has("#orch-col-needs_you >> .orch-show-more")).to_be(false)
	-- The tab's count says them all too.
	expect(app:has('#orch-board-tabs >> role:tab "Running" >> "20"')).to_be(true)
	app:click("#orch-col-running >> .orch-show-more")
	app:frames(20)
	expect(#column(app, "running")).to_be(20)
	expect(app:has("#orch-col-running >> .orch-show-more")).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("board columns: twenty a drawer, then Show n more", function()
	local app = board(1500, 3000)
	add(app, many(25, "planned", "next", "planned", "plan"))
	expect(app:text("#orch-drawer-planned >> .orch-drawer-count")).to_be("29")
	-- (Folded: nothing to show more of.)
	expect(app:has("#orch-drawer-planned >> .orch-show-more")).to_be(false)
	app:click("#orch-drawer-planned >> .orch-drawer-head")
	app:frames(20)
	expect(#drawer(app, "planned")).to_be(20)
	expect(app:text("#orch-drawer-planned >> .orch-show-more")).to_be("Show 9 more")
	app:click("#orch-drawer-planned >> .orch-show-more")
	app:frames(20)
	expect(#drawer(app, "planned")).to_be(29)
	expect(app:has("#orch-drawer-planned >> .orch-show-more")).to_be(false)
	-- (Running's own cards, under its cap, no more.)
	expect(#column(app, "running")).to_be(5)
	expect(app:error()).to_be_nil()
end)

test("board columns: an empty column says so in a line; an empty drawer isn't there", function()
	local app = board()
	expect(app:has(".orch-column-empty")).to_be(false)
	app:set_state("fx_items", {})
	app:set_state("fx_cards", {})
	app:frames(20)
	-- Every column stands, each with its line and a count of nothing.
	local EMPTY: { [string]: string } = { needs_you = "Nothing needs you", running = "Nothing is running", done = "Nothing finished today" }
	for place, words in EMPTY do
		expect(app:has(`#orch-col-{place}`)).to_be(true)
		expect(app:has(`#orch-col-{place} >> .orch-column-empty >> "{words}"`)).to_be(true)
		expect(app:text(`#orch-col-{place} >> .orch-column-count`)).to_be("0")
	end
	for _, d in { "snoozed", "planned", "earlier" } do
		expect(app:has(`#orch-drawer-{d}`)).to_be(false)
	end
	-- One card snoozed: Needs you's line stays, its drawer under it. One
	-- running: Running's line goes.
	add(app, {
		one("it-snz", "snoozed", "asked", "blocked", { ["until"] = T + 30 * MIN }),
		one("it-run", "running", "working", "active"),
	})
	expect(app:has("#orch-col-needs_you >> .orch-column-empty")).to_be(true)
	expect(app:has("#orch-drawer-snoozed")).to_be(true)
	expect(app:has("#orch-col-running >> .orch-column-empty")).to_be(false)
	expect(joined(column(app, "running"))).to_be("it-run")
	expect(app:error()).to_be_nil()
end)

test("board columns: a card with no row shows by its old column", function()
	local app = board()
	local function it(id: string, state: string, extra: { [string]: any }?): any
		local x: { [string]: any } = { id = id, orchestrator_id = "orc-ci", title = id, state = state, priority = "normal", order = 1, threads = {}, created = T - 2 * HOUR, updated = T - HOUR, links = {} }
		for k, v in extra or {} do
			x[k] = v
		end
		return { item = x }
	end
	add(app, {
		it("old-active", "active", { threads = { "th-chat" } }),
		it("old-paused", "blocked", { paused_at = T - HOUR }),
		it("old-review", "review"),
		it("old-done", "done", { done = T - 30 * MIN }),
	})
	-- An ask naming the Review card: the person's to look at.
	local asks = table.clone(app:state("fx_asks") :: { any })
	table.insert(asks, { id = "ak-old", item_id = "old-review", kind = "decision", question = "Look at it?", options = {}, state = "open", created = T - 5 * MIN, quiet = false, images = {}, links = {} })
	app:set_state("fx_asks", asks)
	app:frames(20)
	expect(table.find(column(app, "running"), "old-active") ~= nil).to_be(true)
	expect(app:text("#orch-card-old-active >> .orch-card-status")).to_be("Working")
	expect(table.find(column(app, "needs_you"), "old-review") ~= nil).to_be(true)
	expect(app:text("#orch-card-old-review >> .orch-card-status")).to_be("Ready to look at")
	expect(table.find(column(app, "done"), "old-done") ~= nil).to_be(true)
	app:click("#orch-drawer-planned >> .orch-drawer-head")
	app:frames(20)
	expect(table.find(drawer(app, "planned"), "old-paused") ~= nil).to_be(true)
	expect(app:text("#orch-card-old-paused >> .orch-card-status")).to_be("Paused by you")
	expect(app:error()).to_be_nil()
end)

test("board columns: a row an older engine has overtaken gives way to the item's old column", function()
	local app = board()
	expect(table.find(column(app, "running"), "it-chat") ~= nil).to_be(true)
	-- Closed by a writer that knows no places: its row still says Running.
	patch(app, "fx_items", "it-chat", { state = "done", done = T - 2 * MIN })
	expect(table.find(column(app, "running"), "it-chat")).to_be_nil()
	expect(column(app, "done")[1]).to_be("it-chat")
	expect(app:text("#orch-col-running >> .orch-column-count")).to_be("4")
	expect(app:text("#orch-col-done >> .orch-column-count")).to_be("4")
	expect(app:text("#orch-card-it-chat >> .orch-card-status")).to_be("Done in 48m")
	expect(app:error()).to_be_nil()
end)

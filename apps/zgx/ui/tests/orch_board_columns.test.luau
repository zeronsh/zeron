-- The board's columns (ds/orch_board, ui/orch_board_columns_fixture.luau):
-- Inbox and Blocked stand only while they hold a card, the others taking
-- their room; ten cards a column until all are shown, one choice for every
-- column; Done cleared at once, one Undo putting it all back; and moves
-- (dragged, or the menu's Move to) and the menu as before.

local function board(h: number?): any
	local app = mount("orch_board_columns_fixture.luau")
	app:resize(1440, h or 900)
	app:frames(20)
	return app
end

-- The cards' titles in a column, in order.
local function titles(app: any, col: string): { string }
	local out: { string } = {}
	for line in app:tree(`#cols-{col}`):gmatch("[^\n]+") do
		local t = line:match('^  @%d+ button "([^"]+)"')
		-- (Not its buttons: Show more or less, Clear all done, Later's head.)
		if t and not t:match("^Show ") and t ~= "Clear all done" and not t:match("^Later · ") then
			table.insert(out, t)
		end
	end
	return out
end

-- Whether a column stands (in the tree, a hidden one is there, "hidden").
local function stands(app: any, col: string): boolean
	local first = app:tree(`#cols-{col}`):match("[^\n]+") or ""
	return first ~= "" and not first:match(" hidden%s*$")
end

-- Where a column stands, left to right: its x.
local function x_of(app: any, col: string): number
	local first = app:tree(`#cols-{col}`):match("[^\n]+") or ""
	return tonumber(first:match(" (%-?[%d%.]+),%-?[%d%.]+ [%d%.]+x")) or 0
end

-- A node's width as the tree says it.
local function width(app: any, target: string): number
	local first = app:tree(target):match("[^\n]+") or ""
	return tonumber(first:match("([%d%.]+)x[%d%.]+%s*$")) or 0
end

test("orch board columns: Queued, Now, Review, Done; Blocked between Now and Review only while it holds a card", function()
	local app = board()
	expect(stands(app, "blocked")).to_be(false)
	local order = { "queued", "now", "review", "done" }
	for i, k in order do
		expect(stands(app, k)).to_be(true)
		if i > 1 then
			expect(x_of(app, k) > x_of(app, order[i - 1])).to_be(true)
		end
	end
	expect(app:has("#cols-inbox")).to_be(false)
	expect(app:has("#cols-next")).to_be(false)
	-- The four share the width: each a quarter of it, near enough.
	local four = width(app, "#cols-now")
	app:command("fixture.blocked")
	app:frames(20)
	expect(app:has('#cols-blocked >> role:button "DNS failover between regions"')).to_be(true)
	expect(x_of(app, "blocked") > x_of(app, "now")).to_be(true)
	expect(x_of(app, "blocked") < x_of(app, "review")).to_be(true)
	local five = width(app, "#cols-now")
	expect(five < four - 20).to_be(true)
	-- Moved out of Blocked by its menu: Blocked stands aside again, the
	-- others wider.
	app:right_click('#cols-blocked >> role:button "DNS failover between regions"')
	app:frames(10)
	app:click('role:menuitem "Move to"')
	app:frames(10)
	app:click('role:menuitem "Review"')
	app:frames(20)
	expect(app:state("calls")).to_contain("move:it-blk1:review:")
	expect(stands(app, "blocked")).to_be(false)
	expect(app:has('#cols-review >> role:button "DNS failover between regions"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch board columns: a card moved into an empty Blocked brings it back", function()
	local app = board()
	app:right_click('#cols-queued >> role:button "Phase 2 migration"')
	app:frames(10)
	app:click('role:menuitem "Move to"')
	app:frames(10)
	app:click('role:menuitem "Blocked"')
	app:frames(20)
	expect(app:has('#cols-blocked >> role:button "Phase 2 migration"')).to_be(true)
	-- Dragged out again: gone with it.
	app:drag('#cols-blocked >> role:button "Phase 2 migration"', "#cols-queued")
	app:frames(20)
	expect(app:has('#cols-queued >> role:button "Phase 2 migration"')).to_be(true)
	expect(stands(app, "blocked")).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("orch board columns: ten a column, one Show more for them all, one Show less", function()
	local app = board(1600)
	app:command("fixture.many")
	app:frames(20)
	expect(#titles(app, "now")).to_be(10)
	expect(#titles(app, "done")).to_be(10)
	expect(app:has('#cols-now >> role:button "Show 1 more"')).to_be(true)
	expect(app:has('#cols-done >> role:button "Show 2 more"')).to_be(true)
	expect(app:has("#cols-queued >> .orch-column-more")).to_be(false)
	-- (The column's count says them all.)
	expect(app:has('#cols-done >> .orch-column-count >> "12"')).to_be(true)
	-- Shown in Now: Done shows all of its too.
	app:click('#cols-now >> role:button "Show 1 more"')
	app:frames(20)
	expect(app:state("columns_expanded")).to_be(true)
	expect(#titles(app, "now")).to_be(11)
	expect(#titles(app, "done")).to_be(12)
	expect(app:has('#cols-done >> role:button "Show less"')).to_be(true)
	-- Less in Done: Now's back to ten too.
	app:click('#cols-done >> role:button "Show less"')
	app:frames(20)
	expect(app:state("columns_expanded")).to_be(false)
	expect(#titles(app, "now")).to_be(10)
	expect(#titles(app, "done")).to_be(10)
	expect(app:error()).to_be_nil()
end)

test("orch board columns: Clear takes every Done card off, one Undo puts them all back", function()
	local app = board(1600)
	app:command("fixture.many")
	app:frames(20)
	local chats_before = app:state("columns_chats")
	app:click('#cols-done >> role:button "Clear all done"')
	app:frames(20)
	local calls = app:state("calls") :: string
	for i = 1, 12 do
		expect(calls).to_contain(`remove:it-done{i}`)
	end
	expect(#titles(app, "done")).to_be(0)
	expect(app:has("#cols-done >> .orch-clear-done")).to_be(false)
	expect(app:has('"Cleared 12 done cards"')).to_be(true)
	-- Nothing else asked: no thread opened, no card moved, no chat touched.
	expect(calls).never.to_contain("open:")
	expect(calls).never.to_contain(",move:")
	expect(string.sub(calls, 1, 5)).never.to_be("move:")
	expect(app:state("columns_chats")).to_equal(chats_before)
	app:click('role:button "Undo"')
	app:frames(20)
	calls = app:state("calls") :: string
	for i = 1, 12 do
		expect(calls).to_contain(`restore:it-done{i}`)
	end
	expect(app:has('#cols-done >> .orch-column-count >> "12"')).to_be(true)
	expect(app:has('#cols-done >> role:button "Linux titlebar without controls"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch board columns: an empty Done has nothing to clear", function()
	local app = board()
	app:command("fixture.many")
	app:frames(10)
	app:set_state("columns_items", {})
	app:frames(20)
	expect(app:has(".orch-clear-done")).to_be(false)
end)

test("orch board columns: dragged between columns and the menu, as before", function()
	local app = board()
	app:drag('#cols-queued >> role:button "Write the mesh runbook"', "#cols-review")
	app:frames(20)
	expect(app:state("calls")).to_contain("move:it-next2:review:")
	expect(app:has('#cols-review >> role:button "Write the mesh runbook"')).to_be(true)
	app:right_click('#cols-done >> role:button "Delete cards from the Board"')
	app:frames(10)
	for _, name in { "Move to", "Priority", "Delete" } do
		expect(app:has(`role:menuitem "{name}"`)).to_be(true)
	end
	app:click('role:menuitem "Delete"')
	app:frames(20)
	expect(app:state("calls")).to_contain("remove:it-done2")
	expect(app:error()).to_be_nil()
end)

test("orch board columns: a card dragged within a capped column lands among the ten", function()
	local app = board(1600)
	app:command("fixture.many")
	app:frames(20)
	app:drag('#cols-now >> role:button "Map island layout"', '#cols-now >> role:button "Board columns cap"')
	app:frames(20)
	local calls = app:state("calls") :: string
	local order = tonumber(string.match(calls, "move:it%-now8:active:([%d%.%-]+)"))
	expect(order ~= nil).to_be(true)
	-- (Between the second and the third.)
	expect((order :: number) > 2 and (order :: number) < 3).to_be(true)
	expect(app:error()).to_be_nil()
end)

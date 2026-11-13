-- The board's Queued column (ds/orch_board, ui/orch_board_columns_fixture
-- .luau): Up next (cards `planned`) and, under it, Later (cards `inbox`),
-- folded until opened, its count on its head; a card dragged between the
-- two changes its state; dragged into Now, it starts; Move to offers Up
-- next and Later; ten cards for the column as a whole, one Show more for
-- every column.

local function board(h: number?): any
	local app = mount("orch_board_columns_fixture.luau")
	app:resize(1440, h or 900)
	app:frames(20)
	return app
end

-- What a node's tree line says: whether it's hidden.
local function hidden(app: any, target: string): boolean
	local first = app:tree(target):match("[^\n]+") or ""
	return first:match(" hidden%s*$") ~= nil
end

-- The cards Queued shows, top to bottom (Up next's, then Later's).
local function shown_cards(app: any): { string }
	local out: { string } = {}
	for line in app:tree("#cols-queued"):gmatch("[^\n]+") do
		if not line:match(" hidden%s*$") then
			local t = line:match('^%s+@%d+ button "([^"]+)"')
			if t and not t:match("^Show ") and not t:match("^Later · ") and t ~= "Card actions" then
				table.insert(out, t)
			end
		end
	end
	return out
end

local function has(list: { string }, title: string): boolean
	return table.find(list, title) ~= nil
end

test("orch board queued: planned cards are Up next, inbox cards Later, folded with its count", function()
	local app = board()
	-- No Later cards: no Later head.
	expect(hidden(app, "#cols-queued >> .orch-later-head")).to_be(true)
	app:command("fixture.later")
	app:frames(20)
	-- Queued's head counts Up next (two); Later's head says its four.
	expect(app:has('#cols-queued >> .orch-column-count >> "2"')).to_be(true)
	expect(app:has('#cols-queued >> role:button "Later · 4"')).to_be(true)
	local cards = shown_cards(app)
	expect(has(cards, "Phase 2 migration")).to_be(true)
	expect(has(cards, "Write the mesh runbook")).to_be(true)
	-- Folded: none of Later's cards.
	expect(has(cards, "Look at the hover delay")).to_be(false)
	expect(app:state("columns_later")).to_be(false)
	-- Opened: Later's cards, under Up next's.
	app:click('#cols-queued >> role:button "Later · 4"')
	app:frames(20)
	expect(app:state("columns_later")).to_be(true)
	cards = shown_cards(app)
	expect(#cards).to_be(6)
	expect(cards[1]).to_be("Phase 2 migration")
	expect(cards[3]).to_be("Look at the hover delay")
	-- Folded again.
	app:click('#cols-queued >> role:button "Later · 4"')
	app:frames(20)
	expect(app:state("columns_later")).to_be(false)
	expect(#shown_cards(app)).to_be(2)
	expect(app:error()).to_be_nil()
end)

test("orch board queued: Later stays open as it was left (its state kept)", function()
	local app = board()
	app:set_state("columns_later", true)
	app:command("fixture.later")
	app:frames(20)
	expect(has(shown_cards(app), "Try a darker map")).to_be(true)
end)

test("orch board queued: a Later card dragged into Up next is planned, and back", function()
	local app = board()
	app:command("fixture.later")
	app:set_state("columns_later", true)
	app:frames(20)
	app:drag('#cols-queued >> role:button "Try a darker map"', '#cols-queued >> role:button "Phase 2 migration"')
	app:frames(20)
	expect(app:state("calls")).to_contain("move:it-later2:planned:")
	local cards = shown_cards(app)
	expect(cards[1]).to_be("Try a darker map")
	-- An Up next card dragged onto a Later card: Later.
	app:drag('#cols-queued >> role:button "Write the mesh runbook"', '#cols-queued >> role:button "Someday: a CLI"')
	app:frames(20)
	expect(app:state("calls")).to_contain("move:it-next2:inbox:")
	expect(app:error()).to_be_nil()
end)

test("orch board queued: dragged onto a folded Later's head, a card goes to Later", function()
	local app = board()
	app:command("fixture.later")
	app:frames(20)
	app:drag('#cols-queued >> role:button "Phase 2 migration"', "#cols-queued >> .orch-later-head")
	app:frames(20)
	expect(app:state("calls")).to_contain("move:it-next1:inbox:")
	expect(has(shown_cards(app), "Phase 2 migration")).to_be(false)
	expect(app:has('#cols-queued >> role:button "Later · 5"')).to_be(true)
end)

test("orch board queued: a Queued card dragged into Now starts", function()
	local app = board()
	app:command("fixture.later")
	app:set_state("columns_later", true)
	app:frames(20)
	app:drag('#cols-queued >> role:button "Look at the hover delay"', "#cols-now")
	app:frames(20)
	expect(app:state("calls")).to_contain("start:it-later1:")
	expect(app:has('#cols-now >> role:button "Look at the hover delay"')).to_be(true)
	app:drag('#cols-queued >> role:button "Phase 2 migration"', "#cols-now")
	app:frames(20)
	expect(app:state("calls")).to_contain("start:it-next1:")
	expect(app:error()).to_be_nil()
end)

test("orch board queued: Move to offers Up next and Later, not Next and Inbox", function()
	local app = board()
	app:right_click('#cols-queued >> role:button "Phase 2 migration"')
	app:frames(10)
	app:click('role:menuitem "Move to"')
	app:frames(10)
	for _, name in { "Up next", "Later", "Now", "Blocked", "Review", "Done today" } do
		expect(app:has(`role:menuitem "{name}"`)).to_be(true)
	end
	expect(app:has('role:menuitem "Next"')).to_be(false)
	expect(app:has('role:menuitem "Inbox"')).to_be(false)
	app:click('role:menuitem "Later"')
	app:frames(20)
	expect(app:state("calls")).to_contain("move:it-next1:inbox:")
	expect(app:has('#cols-queued >> role:button "Later · 1"')).to_be(true)
	-- And back to Up next.
	app:set_state("columns_later", true)
	app:frames(10)
	app:right_click('#cols-queued >> role:button "Phase 2 migration"')
	app:frames(10)
	app:click('role:menuitem "Move to"')
	app:frames(10)
	app:click('role:menuitem "Up next"')
	app:frames(20)
	expect(app:state("calls")).to_contain("move:it-next1:planned:")
	expect(hidden(app, "#cols-queued >> .orch-later-head")).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch board queued: ten for the column as a whole, Up next first, Later only while open", function()
	local app = board(1600)
	app:command("fixture.queued")
	app:frames(20)
	-- Folded Later: Up next's eight, nothing more to show.
	expect(#shown_cards(app)).to_be(8)
	expect(app:has("#cols-queued >> .orch-column-more")).to_be(false)
	-- Later open: eight and two of its five, then Show 3 more.
	app:click('#cols-queued >> role:button "Later · 5"')
	app:frames(20)
	local cards = shown_cards(app)
	expect(#cards).to_be(10)
	expect(cards[8]).to_be("Up next thing 8")
	expect(cards[9]).to_be("Later idea 1")
	expect(app:has('#cols-queued >> role:button "Show 3 more"')).to_be(true)
	-- Shown in full: every column, Queued all thirteen.
	app:click('#cols-queued >> role:button "Show 3 more"')
	app:frames(20)
	expect(app:state("columns_expanded")).to_be(true)
	expect(#shown_cards(app)).to_be(13)
	app:click('#cols-queued >> role:button "Show less"')
	app:frames(20)
	expect(app:state("columns_expanded")).to_be(false)
	expect(#shown_cards(app)).to_be(10)
	expect(app:error()).to_be_nil()
end)

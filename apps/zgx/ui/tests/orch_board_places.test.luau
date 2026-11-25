-- The board on the engine's places (ds/orch_board, ui/orch_board_places_fixture.luau):
-- three columns with a drawer each, a card's detail beside them, and the
-- same cards as a list. A first pass: that each piece mounts and shows what
-- the rows say. (The cases owed are listed in the board's hand-off notes.)

test("places board: three columns hold the cards the rows place there", function()
	local app = mount("orch_board_places_fixture.luau")
	app:resize(1500, 1000)
	app:frames(30)
	expect(app:error()).to_be_nil()
	expect(app:count("#orch-col-needs_you >> .orch-card")).to_be(3)
	expect(app:count("#orch-col-running >> .orch-card")).to_be(5)
	expect(app:count("#orch-col-done >> .orch-card")).to_be(3)
	expect(app:has("#orch-drawer-snoozed")).to_be(true)
	expect(app:has("#orch-drawer-planned")).to_be(true)
	expect(app:has("#orch-drawer-earlier")).to_be(true)
end)

test("places board: a drawer opens to its cards", function()
	local app = mount("orch_board_places_fixture.luau")
	app:resize(1500, 1000)
	app:frames(30)
	app:click("#orch-drawer-planned >> .orch-drawer-head")
	app:frames(10)
	expect(app:count("#orch-drawer-planned >> .orch-card")).to_be(4)
	expect(app:error()).to_be_nil()
end)

test("places board: a card opens its detail beside the board, with its ask's buttons and what happened", function()
	local app = mount("orch_board_places_fixture.luau")
	app:resize(1500, 1000)
	app:frames(30)
	app:click("#orch-card-it-merge")
	app:frames(20)
	expect(app:error()).to_be_nil()
	expect(app:has("#orch-board-detail")).to_be(true)
	expect(app:text("#orch-detail-title")).to_be("Bring main (0.3.8) into orchestrators")
	expect(app:has('#orch-detail-needs >> role:button "Unlocked + push the fork"')).to_be(true)
	expect(app:count("#orch-detail-timeline >> .orch-detail-event")).to_be(5)
end)

test("places board: the list view shows the same places as sections", function()
	local app = mount("orch_board_places_fixture.luau")
	app:resize(1500, 1000)
	app:set_state("board_view", "list")
	app:frames(30)
	expect(app:error()).to_be_nil()
	expect(app:count("#orch-list-needs_you >> .orch-list-row")).to_be(3)
	expect(app:count("#orch-list-running >> .orch-list-row")).to_be(5)
	expect(app:count("#orch-list-planned >> .orch-list-row")).to_be(4)
end)

test("places board: the list by itself, at a side panel's width", function()
	local app = mount("orch_board_places_fixture.luau")
	app:resize(900, 1000)
	app:set_state("only_list", true)
	app:set_state("compact", true)
	app:frames(30)
	expect(app:error()).to_be_nil()
	expect(app:count("#fixture-list >> .orch-list-row") >= 12).to_be(true)
end)

-- Symbols typed as text, never in the UI: icons and spacing instead.
local SYMBOLS = { "·", "•", "→", "←", "↗", "✓", "✔", "✗", "✕", "×", "▸", "▾", "›", "»", "└", "+ " }
local function no_symbols(app: any, sel: string)
	local t = app:tree(sel, 40)
	for _, s in SYMBOLS do
		for l in t:gmatch("[^\n]+") do
			-- (A node's own text, not what a card or an ask was given to say.)
			if string.find(l, s, 1, true) and not string.find(l, "Unlocked + push", 1, true) then
				expect(`{sel}: {l}`).to_be(`{sel}: no {s}`)
			end
		end
	end
end

test("places board: no symbol typed as text, on the board, a detail, the list or New task", function()
	local app = mount("orch_board_places_fixture.luau")
	app:resize(1700, 1200)
	app:set_state("board_drawers", { snoozed = true, planned = true, earlier = true })
	app:set_state("board_selected", "it-merge")
	app:frames(30)
	no_symbols(app, "#orch-board")
	app:set_state("board_selected", "it-asks")
	app:frames(20)
	no_symbols(app, "#orch-board-detail")
	app:set_state("board_view", "list")
	app:frames(20)
	no_symbols(app, "#orch-list")
	app:click("#orch-board-new")
	app:frames(30)
	expect(app:has("#orch-new-task")).to_be(true)
	no_symbols(app, "#orch-new-task")
	expect(app:error()).to_be_nil()
end)

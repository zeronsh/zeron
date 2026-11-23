-- What a board card costs (ds/orch_board, on 200 cards:
-- ui/orch_board_perf_fixture.luau): a push that changes one card wakes
-- that card alone, and one that changes nothing wakes none; what shows now
-- and then (a card's menu, a paused card's Resume, an ask's buttons in the
-- list) is built only where it shows. (tests/orch_board_perf.test.luau
-- times the same pushes.)

local function open(view: string?): any
	local app = mount("orch_board_perf_fixture.luau")
	app:resize(1500, 1400)
	if view then
		app:set_state("board_view", view)
	end
	app:frames(30)
	return app
end

local function counts(app: any): { [string]: number }
	app:command("perf_read")
	return app:state("perf_counts") or {}
end

local function push(app: any, command: string): { [string]: number }
	app:command("perf_reset")
	app:command(command)
	app:frames(10)
	return counts(app)
end

test("board cards: a push that changes one card wakes that card alone", function()
	local app = open()
	expect(app:count("#orch-board >> .orch-card") > 30).to_be(true)
	local c = push(app, "perf_status")
	-- (Each card's words are worked out again only if its card changed.)
	expect(c["board.word"] or 0).to_be(1)
	expect(c["board.card_entry"] or 0).to_be(1)
	expect(app:text("#orch-card-it-37 >> .orch-card-status")).to_be("Waiting on tests")
	-- The same rows again, in a new list or each a new table: no card.
	for _, same in { "perf_same", "perf_same_fresh" } do
		local c2 = push(app, same)
		expect(c2["board.card_entry"] or 0).to_be(0)
		expect(c2["board.word"] or 0).to_be(0)
		expect(c2["board.ids"] or 0).to_be(0)
	end
	expect(app:error()).to_be_nil()
end)

test("board cards: in the list, a push that changes one card wakes its row alone", function()
	local app = open("list")
	expect(app:count("#orch-list >> .orch-list-row") > 100).to_be(true)
	local c = push(app, "perf_status")
	expect(c["board.row_entry"] or 0).to_be(1)
	expect(c["board.row"] or 0).to_be(0)
	local c2 = push(app, "perf_same_fresh")
	expect(c2["board.row_entry"] or 0).to_be(0)
	expect(app:error()).to_be_nil()
end)

test("board cards: a card's menu, a paused card's Resume and an ask's buttons are built only where they show", function()
	local app = open()
	-- No menu is made until one opens, and then that card's alone.
	expect(counts(app)["board.menu"] or 0).to_be(0)
	expect(app:count("role:menuitem")).to_be(0)
	app:command("perf_reset")
	app:right_click("#orch-card-it-37")
	app:frames(30)
	expect(counts(app)["board.menu"] or 0).to_be(1)
	expect(app:count("role:menuitem") > 0).to_be(true)
	app:key("Escape")
	app:frames(10)
	-- Resume under the paused cards alone, and only once Planned is open.
	expect(app:count("#orch-board >> .orch-card-resume")).to_be(0)
	app:click("#orch-drawer-planned >> .orch-drawer-head")
	app:frames(10)
	-- (The paused come after what starts next: past the drawer's first 20.)
	expect(app:count("#orch-drawer-planned >> .orch-card-resume")).to_be(0)
	app:scroll("#fixture-root", 3000)
	app:frames(10)
	app:click("#orch-drawer-planned >> .orch-show-more")
	app:frames(10)
	local planned = app:count("#orch-drawer-planned >> .orch-card")
	local resumes = app:count("#orch-drawer-planned >> .orch-card-resume")
	expect(resumes > 0).to_be(true)
	expect(resumes < planned).to_be(true)
	-- In the list: an ask's buttons under the rows that need the person
	-- alone (Snoozed folded).
	app:set_state("board_view", "list")
	app:frames(10)
	expect(app:count("#orch-list >> .orch-list-ask")).to_be(28)
	expect(app:count("#orch-list >> .orch-list-row") > 28).to_be(true)
	expect(app:error()).to_be_nil()
end)

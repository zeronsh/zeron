-- What a Board card costs (ds/orch_board): a push that changes one item
-- wakes that card alone; what shows now and then (a decision's chips,
-- Review's buttons, the box for changes, the "…" menu) is built the first
-- time it shows; and the Board shown builds its cards a few a frame, the
-- top of each column first (ds/stage's `Idle`, `URGENT_PER_FRAME`). On
-- ui/control_page_fixture.luau (ui/control_world).

local function open(): any
	local app = mount("control_page_fixture.luau")
	app:resize(1440, 1400)
	app:set_state("control_tab", "board")
	app:frames(60)
	return app
end

local function counts(app: any): { [string]: number }
	app:command("fixture.perf_read")
	return app:state("perf_counts") or {}
end

test("board cards: a push that changes one item wakes that card alone", function()
	local app = open()
	local n = app:count("#control-board-tab >> .orch-card")
	expect(n > 3).to_be(true)
	app:command("fixture.perf_reset")
	app:command("fixture.push_one")
	app:frames(20)
	-- (Each card's word is worked out again only if its item changed.)
	expect(counts(app)["board.word"] or 0).to_be(1)
	expect(app:has('#control-board-tab >> .orch-card "Reboot Moscow node tonight"')).to_be(true)
	-- The same push again, nothing changed: no card at all.
	app:command("fixture.perf_reset")
	app:command("fixture.push_one")
	app:frames(20)
	expect(counts(app)["board.word"] or 0).to_be(0)
	expect(app:error()).to_be_nil()
end)

test("board cards: the menu, the box for changes, Review's buttons and a decision's chips are built only where they show", function()
	local app = open()
	local cards = app:count("#control-board-tab >> .orch-card")
	-- No "…" until the pointer rests on a card.
	expect(app:count("#control-board-tab >> .orch-card-menu")).to_be(0)
	app:hover('#control-board-tab >> .orch-card "Reboot Moscow node"')
	app:frames(10)
	expect(app:count("#control-board-tab >> .orch-card-menu")).to_be(1)
	-- Review's buttons on the Review cards alone; no box for changes yet.
	local review = app:count("#control-board-review >> .orch-card")
	expect(app:count("#control-board-tab >> .orch-card-review")).to_be(review)
	expect(review < cards).to_be(true)
	expect(app:count("#control-board-tab >> .orch-card-changes")).to_be(0)
	app:click('#control-board-review >> .orch-card [1] >> role:button "Request changes…"')
	app:frames(10)
	expect(app:count("#control-board-tab >> .orch-card-changes")).to_be(1)
	-- A decision's chips on the cards one waits on alone, more as
	-- others come.
	local asked = app:count("#control-board-tab >> .orch-card-choices")
	expect(asked < cards).to_be(true)
	app:command("fixture.decision")
	app:frames(30)
	expect(app:count("#control-board-tab >> .orch-card-choices") > asked).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("board cards: the Board shown builds its cards a few a frame, the top of each column first", function()
	local app = mount("control_page_fixture.luau")
	app:resize(1440, 1400)
	app:set_state("control_tab", "board")
	local last, most, first_built = 0, 0, nil
	for i = 1, 120 do
		app:frames(1)
		local n = app:count("#control-board-tab >> .orch-card")
		most = math.max(most, n - last)
		if n > 0 and not first_built then
			first_built = i
			-- (The first to come: a column's first card.)
			expect(app:count("#control-board-tab >> .orch-column >> .orch-card-slot [1] >> .orch-card") >= 1).to_be(true)
		end
		last = n
	end
	expect(last > 3).to_be(true)
	expect(most <= 3).to_be(true)
	expect(app:count("#control-board-tab >> .stage-idle")).to_be(0)
	expect(app:error()).to_be_nil()
end)

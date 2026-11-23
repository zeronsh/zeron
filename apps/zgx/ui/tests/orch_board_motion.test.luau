-- How the board moves (ds/orch_board_motion; step 3 of the board's
-- design): a move's three beats (its status at once, a hold where it was,
-- then its travel), the cap of three cards in flight, Reduce Motion, what
-- is no move, a card moved again on its way, who is told (a toast for the
-- person's moves, the unseen dot for the engine's), the drawers, the
-- detail, and the board turning into its list. On the 200-card fixture
-- (ui/orch_board_perf_fixture.luau) and the places one
-- (ui/orch_board_places_fixture.luau).
--
-- Frames are 1/60 s of the test's time and timers fire in the frame their
-- time falls in, so the beats' timing is held exactly: the hold is 800 ms
-- (48 frames), the travel 350 ms (21). The frames' own cost while cards
-- move is printed (`zeron-zgx: BOARDMOTION …`), not held: it is this
-- machine's.

local HOLD, TRAVEL, FADE = 48, 21, 8

local function open(fixture: string?, w: number?, h: number?): any
	local app = mount(fixture or "orch_board_perf_fixture.luau")
	app:resize(w or 1500, h or 1400)
	app:frames(30)
	expect(app:error()).to_be_nil()
	return app
end

local function motion(app: any): any
	return app:state("orch_board_motion") or {}
end
local function phase(app: any, id: string): string?
	return (motion(app).phase or {})[id]
end
local function in_col(app: any, place: string, id: string): boolean
	return app:has(`#orch-col-{place} >> #orch-card-{id}`)
end
local function col_count(app: any, place: string): string
	return app:text(`#orch-col-{place} >> .orch-column-count`)
end
local function tab_says(app: any, label: string, n: number): boolean
	return app:has(`#orch-board-tabs >> role:tab "{label}" >> "{n}"`)
end
-- Nothing in flight, no timer waiting.
local function settled(app: any): boolean
	local m = motion(app)
	return m.flying == 0 and m.timers == 0 and next(m.phase or {}) == nil
end

test("board motion: a move's three beats, the tabs at once and the column heads on landing", function()
	local app = open()
	expect(in_col(app, "running", "it-37")).to_be(true)
	expect(col_count(app, "running")).to_be("64")
	expect(col_count(app, "done")).to_be("30")
	app:command("motion_one")
	app:frames(1)
	-- Beat 1: its status in place, the tabs' counts at once.
	expect(app:text("#orch-card-it-37 >> .orch-card-status")).to_contain("Done")
	expect(tab_says(app, "Running", 63)).to_be(true)
	expect(tab_says(app, "Done", 31)).to_be(true)
	-- Beat 2: held in Running, its heads' counts as they were.
	expect(in_col(app, "running", "it-37")).to_be(true)
	expect(in_col(app, "done", "it-37")).to_be(false)
	expect(col_count(app, "running")).to_be("64")
	expect(col_count(app, "done")).to_be("30")
	expect(phase(app, "it-37")).to_be("hold")
	expect(motion(app).flying).to_be(1)
	app:frames(HOLD - 3)
	expect(in_col(app, "running", "it-37")).to_be(true)
	expect(phase(app, "it-37")).to_be("hold")
	-- Beat 3: at 800 ms it is presented in Done (the travel), the heads
	-- counting it there.
	app:frames(3)
	expect(in_col(app, "done", "it-37")).to_be(true)
	expect(in_col(app, "running", "it-37")).to_be(false)
	expect(col_count(app, "running")).to_be("63")
	expect(col_count(app, "done")).to_be("31")
	expect(phase(app, "it-37")).to_be("travel")
	-- Landed at 1150 ms: nothing in flight.
	app:frames(TRAVEL)
	expect(settled(app)).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("board motion: at most three in flight; a fourth crossfades", function()
	local app = open()
	app:command("motion_four")
	app:frames(1)
	expect(motion(app).flying).to_be(3)
	expect(phase(app, "it-38")).to_be("hold")
	expect(phase(app, "it-39")).to_be("hold")
	expect(phase(app, "it-40")).to_be("hold")
	expect(phase(app, "it-41")).to_be("fade_out")
	-- The fourth: out where it was, then in where it goes, no hold.
	app:frames(FADE + 1)
	expect(in_col(app, "done", "it-41")).to_be(true)
	expect(phase(app, "it-41")).to_be("fade_in")
	expect(in_col(app, "running", "it-38")).to_be(true)
	app:frames(FADE + 1)
	expect(phase(app, "it-41")).to_be_nil()
	app:frames(HOLD + TRAVEL)
	expect(in_col(app, "done", "it-38")).to_be(true)
	expect(settled(app)).to_be(true)
	-- Ten at once: three travel, seven crossfade.
	local app2 = open()
	app2:command("motion_many")
	app2:frames(1)
	local fades = 0
	for _, p in motion(app2).phase do
		if p == "fade_out" then
			fades += 1
		end
	end
	expect(motion(app2).flying).to_be(3)
	expect(fades).to_be(7)
	app2:frames(HOLD + TRAVEL + 2)
	expect(settled(app2)).to_be(true)
	expect(app2:error()).to_be_nil()
end)

test("board motion: with Reduce Motion every move crossfades", function()
	local app = open()
	app:set_state("motion_reduce", true)
	app:frames(2)
	app:command("motion_one")
	app:frames(1)
	expect(phase(app, "it-37")).to_be("fade_out")
	expect(motion(app).flying).to_be(0)
	app:frames(FADE + 1)
	expect(in_col(app, "done", "it-37")).to_be(true)
	expect(col_count(app, "done")).to_be("31")
	app:frames(FADE + 1)
	expect(settled(app)).to_be(true)
	-- (The board to its list: no box morphs.)
	app:set_state("board_view", "list")
	app:frames(1)
	expect(#(motion(app).morphed or {})).to_be(0)
	expect(app:error()).to_be_nil()
end)

test("board motion: no animation on mount, a tab, a filter, or a row with seq 0", function()
	local app = open()
	expect(settled(app)).to_be(true)
	-- A row mapped from an old column: there at once.
	app:command("motion_seq0")
	app:frames(1)
	expect(in_col(app, "done", "it-42")).to_be(true)
	expect(col_count(app, "done")).to_be("31")
	expect(settled(app)).to_be(true)
	-- A tab.
	app:set_state("board_tab", "running")
	app:frames(1)
	expect(settled(app)).to_be(true)
	app:set_state("board_tab", "all")
	app:frames(2)
	-- A filter (the places fixture's, kept by its host).
	local places = open("orch_board_places_fixture.luau")
	expect(settled(places)).to_be(true)
	places:set_state("filter", "orc-vpn")
	places:frames(1)
	expect(settled(places)).to_be(true)
	places:set_state("filter", nil)
	places:frames(1)
	expect(settled(places)).to_be(true)
	expect(places:error()).to_be_nil()
end)

test("board motion: moved again in its hold, it holds again once, then goes where it stands", function()
	local app = open()
	app:command("motion_one")
	app:frames(10)
	-- To Running again, then Done again: one more hold, not two.
	app:command("motion_back")
	app:frames(10)
	app:command("motion_one")
	app:frames(1)
	expect(phase(app, "it-37")).to_be("hold")
	-- (Past the first hold's end: still held, the hold having begun again.)
	app:frames(HOLD - 20)
	expect(phase(app, "it-37")).to_be("hold")
	expect(in_col(app, "running", "it-37")).to_be(true)
	-- (The second hold ends 800 ms after the first move again.)
	app:frames(12)
	expect(in_col(app, "done", "it-37")).to_be(true)
	expect(phase(app, "it-37")).to_be("travel")
	app:frames(TRAVEL)
	expect(settled(app)).to_be(true)
	-- Moved back during its hold: it stays, lit again.
	local app2 = open()
	app2:command("motion_one")
	app2:frames(10)
	app2:command("motion_back")
	app2:frames(HOLD + 2)
	expect(in_col(app2, "running", "it-37")).to_be(true)
	expect(settled(app2)).to_be(true)
end)

test("board motion: moved again in its travel, it goes on from where it shows", function()
	local app = open()
	app:command("motion_one")
	app:frames(HOLD + 4)
	expect(phase(app, "it-37")).to_be("travel")
	expect(in_col(app, "done", "it-37")).to_be(true)
	app:command("motion_back")
	app:frames(1)
	-- (No new hold: retargeted at once.)
	expect(in_col(app, "running", "it-37")).to_be(true)
	expect(phase(app, "it-37")).to_be("travel")
	app:frames(TRAVEL + 1)
	expect(settled(app)).to_be(true)
	expect(app:error()).to_be_nil()
end)

-- Toasts shown so far (ds/toast counts each one made).
local function toasts(app: any): number
	return app:state("ds_toasts_next") or 0
end
local function places_col(app: any, place: string, id: string): boolean
	return app:has(`#orch-col-{place} >> #orch-card-{id}`)
end

test("board motion: an engine move never toasts; the person's toasts once, at its first beat", function()
	local app = open("orch_board_places_fixture.luau")
	local before = toasts(app)
	app:command("engine_done")
	app:frames(HOLD + TRAVEL + 4)
	expect(places_col(app, "done", "it-chat")).to_be(true)
	expect(toasts(app)).to_be(before)
	-- The person's answer (from the card's detail, past its Undo).
	app:click("#orch-card-it-merge")
	app:frames(20)
	app:click('#orch-detail-needs >> role:button "Unlocked + push the fork"')
	expect(app:wait('role:button "Undo"', { gone = true, timeout_ms = 8000 })).to_be(true)
	app:frames(2)
	-- Said with the move's first beat: the card still holds in Needs you.
	expect(toasts(app) - before).to_be(1)
	expect(places_col(app, "needs_you", "it-merge")).to_be(true)
	app:frames(HOLD + TRAVEL + 4)
	expect(places_col(app, "running", "it-merge")).to_be(true)
	-- (Once: its landing says nothing more.)
	expect(toasts(app) - before).to_be(1)
	expect(app:error()).to_be_nil()
end)

test("board motion: an engine move to Done carries the unseen dot; the person's doesn't", function()
	local app = open()
	app:command("motion_one")
	app:frames(1)
	-- (Its status, and its dot, change in place at once.)
	expect(app:has("#orch-card-it-37 >> .orch-card-unseen")).to_be(true)
	app:frames(HOLD + TRAVEL + 2)
	expect(app:has("#orch-col-done >> #orch-card-it-37 >> .orch-card-unseen")).to_be(true)
	local places = open("orch_board_places_fixture.luau")
	places:right_click("#orch-card-it-chat")
	places:frames(10)
	places:click('role:menuitem "Mark done"')
	places:frames(HOLD + TRAVEL + 4)
	expect(places_col(places, "done", "it-chat")).to_be(true)
	expect(places:has("#orch-card-it-chat >> .orch-card-unseen")).to_be(false)
end)

local function drawer_count(app: any, place: string): string
	return app:text(`#orch-drawer-{place} >> .orch-drawer-count`)
end

test("board motion: into a folded drawer by its head line, out of one from it", function()
	local app = open()
	expect(drawer_count(app, "planned")).to_be("40")
	app:command("motion_pause")
	app:frames(1)
	expect(phase(app, "it-37")).to_be("hold")
	app:frames(HOLD + 1)
	-- To its head: still in Running while it goes, the drawer's count as was.
	expect(phase(app, "it-37")).to_be("out")
	expect(in_col(app, "running", "it-37")).to_be(true)
	expect(drawer_count(app, "planned")).to_be("40")
	app:frames(TRAVEL + 1)
	-- Landed: in the drawer (folded: no card), counted there.
	expect(in_col(app, "running", "it-37")).to_be(false)
	expect(app:has("#orch-card-it-37")).to_be(false)
	expect(drawer_count(app, "planned")).to_be("41")
	expect(settled(app)).to_be(true)
	-- Out of it: comes in at Running's top once its hold is over.
	app:command("motion_start")
	app:frames(1)
	expect(phase(app, "it-101")).to_be("hold")
	expect(drawer_count(app, "planned")).to_be("41")
	app:frames(HOLD + 1)
	expect(in_col(app, "running", "it-101")).to_be(true)
	expect(drawer_count(app, "planned")).to_be("40")
	expect(phase(app, "it-101")).to_be("travel")
	app:frames(TRAVEL + 1)
	expect(settled(app)).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("board motion: a drawer opens and closes, its cards going under its edge", function()
	local app = open()
	expect(app:count("#orch-drawer-planned >> .orch-card")).to_be(0)
	app:set_state("board_drawers", { planned = true })
	app:frames(2)
	expect(app:count("#orch-drawer-planned >> .orch-card") > 0).to_be(true)
	app:set_state("board_drawers", {})
	-- (Fading under its closing edge for 200 ms, then gone.)
	app:frames(2)
	expect(app:count("#orch-drawer-planned >> .orch-card") > 0).to_be(true)
	app:frames(16)
	expect(app:count("#orch-drawer-planned >> .orch-card")).to_be(0)
	expect(app:error()).to_be_nil()
end)

test("board motion: the detail opens, and closes in 130 ms", function()
	local app = open()
	app:click("#orch-card-it-38")
	app:frames(1)
	expect(app:has("#orch-board-detail")).to_be(true)
	app:frames(12)
	app:set_state("board_selected", nil)
	app:frames(3)
	-- (Going: still there, fading.)
	expect(app:has("#orch-board-detail")).to_be(true)
	app:frames(8)
	expect(app:has("#orch-board-detail")).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("board motion: the board to its list morphs the rows in view, at most 24", function()
	-- (Tall: more rows in view than the cap.)
	local app = open(nil, 1500, 2600)
	app:set_state("board_view", "list")
	app:frames(1)
	local morphed = motion(app).morphed or {}
	expect(#morphed).to_be(24)
	for _, id in morphed do
		-- Each is a row built (rows are built near the view alone).
		expect(app:has(`#orch-row-{id}`)).to_be(true)
	end
	-- (Done's first row is far below: not built, not morphed.)
	expect(app:has("#orch-row-it-141")).to_be(false)
	expect(table.find(morphed, "it-141")).to_be_nil()
	app:frames(30)
	-- And back: the cards whose rows showed, at most 24.
	app:set_state("board_view", "board")
	app:frames(1)
	local back = motion(app).morphed or {}
	expect(#back > 0).to_be(true)
	expect(#back <= 24).to_be(true)
	app:frames(30)
	expect(settled(app)).to_be(true)
	-- A window's worth fewer than the cap: each row in view, no more.
	local small = open()
	small:set_state("board_view", "list")
	small:frames(1)
	local few = motion(small).morphed or {}
	expect(#few > 0).to_be(true)
	expect(#few <= 24).to_be(true)
	expect(small:error()).to_be_nil()
end)

-- The worst frame (ms) of `n` frames, each timed.
local function worst(app: any, n: number): (number, number)
	local most, sum = 0, 0
	for _ = 1, n do
		local t0 = os.clock()
		app:frames(1)
		local ms = (os.clock() - t0) * 1000
		most = math.max(most, ms)
		sum += ms
	end
	return most, sum / n
end

test("board motion: frames while cards move, and none asked for once they land", function()
	local cases = { { "one", "motion_one" }, { "three", "motion_three" }, { "four", "motion_four" }, { "many", "motion_many" }, { "answer", "motion_answer" } }
	for _, case in cases do
		local name, command = case[1], case[2]
		local maxes, means = {}, {}
		for _ = 1, 3 do
			local app = open()
			local t0 = os.clock()
			app:command(command)
			local push = (os.clock() - t0) * 1000
			local most, mean = worst(app, HOLD + TRAVEL + 6)
			table.insert(maxes, math.max(most, push))
			table.insert(means, mean)
			-- Settled: nothing in flight, no timer, and no frame of the board's
			-- wanted (src/board_motion_tests.rs holds that against the frame
			-- clock itself).
			expect(settled(app)).to_be(true)
			expect(app:error()).to_be_nil()
		end
		table.sort(maxes)
		table.sort(means)
		print(`zeron-zgx: BOARDMOTION {name} worst-frame={string.format("%.2f", maxes[#maxes])}ms median-run-worst={string.format("%.2f", maxes[2])}ms mean-frame={string.format("%.2f", means[2])}ms`)
	end
end)

test("board motion: the switch to the list and back, timed with the morph", function()
	local to_list, to_board = {}, {}
	for _ = 1, 5 do
		local app = open()
		local t0 = os.clock()
		app:set_state("board_view", "list")
		app:frames(1)
		table.insert(to_list, (os.clock() - t0) * 1000)
		app:frames(30)
		local t1 = os.clock()
		app:set_state("board_view", "board")
		app:frames(1)
		table.insert(to_board, (os.clock() - t1) * 1000)
		local most = worst(app, 20)
		table.insert(to_board, most)
		expect(app:error()).to_be_nil()
	end
	table.sort(to_list)
	table.sort(to_board)
	print(`zeron-zgx: BOARDMOTION to-list p50={string.format("%.2f", to_list[3])}ms max={string.format("%.2f", to_list[#to_list])}ms`)
	print(`zeron-zgx: BOARDMOTION to-board p50={string.format("%.2f", to_board[#to_board // 2 + 1])}ms max={string.format("%.2f", to_board[#to_board])}ms`)
end)

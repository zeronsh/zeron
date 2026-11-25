-- What one card's change costs on a 200-card board (ds/orch_board,
-- ui/orch_board_perf_fixture.luau): a status change in its column, a move
-- between columns, a minute going by; on the board and in its list. Prints
-- its numbers (`BOARDPERF …`: each push and the frame after it, p50 and
-- max, and the run counts per push, ds/orch_perf_trace's `board.*`), and
-- holds what a push may wake: a status change, its own card alone; a move,
-- that and the two columns' lists and the tabs' counts; a minute, the
-- clocks shown. (Times are printed, not held.)

local N = 20

local function pct(list: { number }, p: number): number
	local s = table.clone(list)
	table.sort(s)
	return s[math.clamp(math.ceil(#s * p), 1, #s)]
end

local function open(view: string?): any
	local app = mount("orch_board_perf_fixture.luau")
	app:resize(1500, 1400)
	if view then
		app:set_state("board_view", view)
	end
	app:frames(30)
	return app
end

-- `n` pushes made by `command`, each and the frame after it timed (ms);
-- the run counts per push.
local function pushes(app: any, command: string, n: number, between: number?): ({ number }, { [string]: number })
	app:command("perf_reset")
	local out = {}
	for _ = 1, n do
		local t0 = os.clock()
		app:command(command)
		app:frames(1)
		table.insert(out, (os.clock() - t0) * 1000)
		-- (A move: its hold and travel over before the next.)
		app:frames(between or 5)
	end
	app:command("perf_read")
	local per: { [string]: number } = {}
	for k, v in app:state("perf_counts") or {} do
		per[k] = v / n
	end
	return out, per
end

local function per_push(c: { [string]: number }, key: string): number
	return c[key] or 0
end

local function report(name: string, list: { number }, per: { [string]: number })
	local parts = {}
	for k, v in per do
		table.insert(parts, `{k}={v}`)
	end
	table.sort(parts)
	print(`zeron-zgx: BOARDPERF {name} p50={string.format("%.2f", pct(list, 0.5))}ms max={string.format("%.2f", pct(list, 1))}ms per-push: {table.concat(parts, " ")}`)
end

test("board perf: the fixture shows 200 cards over every place", function()
	local app = open()
	expect(app:error()).to_be_nil()
	expect(app:count("#orch-col-running >> .orch-card")).to_be(12)
	expect(app:has("#orch-card-it-37")).to_be(true)
	expect(app:text("#orch-col-running >> .orch-column-count")).to_be("64")
	-- A push shows: its status in place, then a move.
	app:command("perf_status")
	app:frames(3)
	expect(app:text("#orch-card-it-37 >> .orch-card-status")).to_be("Waiting on tests")
	app:command("perf_move")
	-- (It holds in Running, then travels: tests/orch_board_motion.)
	app:frames(3)
	expect(app:has("#orch-col-running >> #orch-card-it-37")).to_be(true)
	app:frames(75)
	expect(app:has("#orch-col-done >> #orch-card-it-37")).to_be(true)
	expect(app:has("#orch-col-running >> #orch-card-it-37")).to_be(false)
	expect(app:text("#orch-col-running >> .orch-column-count")).to_be("63")
	expect(app:text("#orch-col-done >> .orch-column-count")).to_be("31")
	expect(app:error()).to_be_nil()
end)

test("board perf: a status change in its column", function()
	local app = open()
	local t, c = pushes(app, "perf_status", N)
	report("board status", t, c)
	expect(app:error()).to_be_nil()
	-- Its card alone reads its entry again; no column's list, no count.
	expect(per_push(c, "board.card_entry")).to_be(1)
	expect(per_push(c, "board.word")).to_be(1)
	expect(per_push(c, "board.ids")).to_be(0)
	expect(per_push(c, "board.counts")).to_be(0)
	expect(per_push(c, "board.card")).to_be(0)
end)

test("board perf: a status change, every row a new table", function()
	local app = open()
	local t, c = pushes(app, "perf_status_fresh", N)
	report("board status-fresh", t, c)
	expect(app:error()).to_be_nil()
	expect(per_push(c, "board.card_entry")).to_be(1)
	expect(per_push(c, "board.ids")).to_be(0)
	expect(per_push(c, "board.counts")).to_be(0)
end)

test("board perf: a move between columns", function()
	local app = open()
	local t, c = pushes(app, "perf_move", N, 75)
	report("board move", t, c)
	expect(app:error()).to_be_nil()
	-- The two columns' lists and the tabs' counts; the card changing in
	-- place (its move's first beat), then built where it lands, and the
	-- one that comes into view under Running's cap.
	expect(per_push(c, "board.ids") <= 2).to_be(true)
	expect(per_push(c, "board.counts") <= 4).to_be(true)
	expect(per_push(c, "board.card") <= 2).to_be(true)
	expect(per_push(c, "board.card_entry") <= 3).to_be(true)
end)

test("board perf: a minute goes by", function()
	local app = open()
	local t, c = pushes(app, "perf_tick", N)
	report("board tick", t, c)
	expect(app:error()).to_be_nil()
	-- The clocks shown, nothing else.
	expect(per_push(c, "board.tail") <= app:count("#orch-board >> .orch-card-clock")).to_be(true)
	expect(per_push(c, "board.card_entry")).to_be(0)
	expect(per_push(c, "board.ids")).to_be(0)
end)

test("board perf: a status change and a move, in the list", function()
	local app = open("list")
	-- (Running's tab: its rows at the top, built.)
	app:set_state("board_tab", "running")
	app:frames(10)
	expect(app:has("#orch-row-it-37")).to_be(true)
	local t, c = pushes(app, "perf_status", N)
	report("list status", t, c)
	expect(per_push(c, "board.row_entry")).to_be(1)
	expect(per_push(c, "board.ids")).to_be(0)
	local t2, c2 = pushes(app, "perf_move", N, 75)
	report("list move", t2, c2)
	expect(per_push(c2, "board.ids") <= 2).to_be(true)
	expect(per_push(c2, "board.row") <= 2).to_be(true)
	expect(per_push(c2, "board.row_entry") <= 3).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("board perf: mounting the board, and switching to the list", function()
	local mounts, lists = {}, {}
	local per: { [string]: number } = {}
	for _ = 1, 5 do
		local t0 = os.clock()
		local app = mount("orch_board_perf_fixture.luau")
		app:resize(1500, 1400)
		app:frames(1)
		table.insert(mounts, (os.clock() - t0) * 1000)
		app:frames(10)
		app:command("perf_reset")
		local t1 = os.clock()
		app:set_state("board_view", "list")
		app:frames(1)
		table.insert(lists, (os.clock() - t1) * 1000)
		app:command("perf_read")
		per = app:state("perf_counts") or {}
		expect(app:error()).to_be_nil()
	end
	report("board mount", mounts, {})
	report("board to-list", lists, per)
end)

-- The list builds the rows near the view alone (ds/orch_board_list's
-- slots): switching to it builds what a screen shows, in one frame; a
-- scroll builds the rows it brings near and lets go of those it leaves.
local function built_rows(app: any): number
	return app:count("#orch-list >> .orch-list-row")
end

test("board perf: the list builds the rows in view, and a scroll the rows it brings", function()
	local times, rows = {}, 0
	for _ = 1, 10 do
		local app = open()
		app:command("perf_reset")
		local t0 = os.clock()
		app:set_state("board_view", "list")
		app:frames(1)
		table.insert(times, (os.clock() - t0) * 1000)
		app:command("perf_read")
		local c = app:state("perf_counts") or {}
		rows = c["board.row"] or 0
		-- At most a 1400-high window's rows (and the margin past it): some
		-- 40 lines of 30, fewer with asks; never the list's 118.
		expect(rows > 10).to_be(true)
		expect(rows <= 60).to_be(true)
		expect(app:error()).to_be_nil()
	end
	report("board to-list (windowed)", times, { ["board.row"] = rows })
	local app = open("list")
	local first = built_rows(app)
	expect(first <= 60).to_be(true)
	expect(app:has("#orch-row-it-1")).to_be(true)
	-- Done's rows are far below (it-141, Done's first): not built.
	expect(app:has("#orch-row-it-141")).to_be(false)
	-- Scrolled down, they are; the top's are let go.
	for _ = 1, 8 do
		app:scroll("#fixture-root", 1000)
		app:frames(3)
	end
	expect(app:has("#orch-row-it-141")).to_be(true)
	expect(app:has("#orch-row-it-1")).to_be(false)
	expect(built_rows(app) <= 70).to_be(true)
	-- And back up: the top's built again, at the height they held.
	app:scroll("#fixture-root", -20000)
	app:frames(5)
	expect(app:has("#orch-row-it-1")).to_be(true)
	expect(app:error()).to_be_nil()
end)

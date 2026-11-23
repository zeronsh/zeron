-- What one change costs on the Control room with a big fleet
-- (ui/orch_perf_fixture.luau: 100 threads, a Board card a crew thread,
-- Progress all shown): a thread's status, a progress row's step. Prints
-- its numbers (`ORCHPERF …`).

local function pct(list: { number }, p: number): number
	local s = table.clone(list)
	table.sort(s)
	return s[math.clamp(math.ceil(#s * p), 1, #s)]
end

local function open(only: string?): any
	local app = mount("orch_perf_fixture.luau")
	app:resize(1440, 2400)
	if only then
		app:command(`perf_only_{only}`)
	end
	app:command("perf_big")
	app:frames(120)
	return app
end

-- `n` changes made by `command`, each and the frame after timed (ms); the
-- run counts over them all.
local function changes(app: any, command: string, n: number): ({ number }, { [string]: number })
	app:command("perf_reset")
	local out = {}
	for _ = 1, n do
		local t0 = os.clock()
		app:command(command)
		app:frames(1)
		table.insert(out, (os.clock() - t0) * 1000)
		app:frames(30)
	end
	app:command("perf_read")
	return out, app:state("perf_counts") or {}
end

local function report(name: string, list: { number }, counts: { [string]: number }, n: number)
	local parts = {}
	for k, v in counts do
		table.insert(parts, `{k}={v / n}`)
	end
	table.sort(parts)
	print(`zeron-zgx: ORCHPERF {name} p50={string.format("%.1f", pct(list, 0.5))}ms max={string.format("%.1f", pct(list, 1))}ms per-change: {table.concat(parts, " ")}`)
end

test("perf: one thread's status change, the whole page", function()
	local app = open()
	local t, c = changes(app, "perf_status", 10)
	report("page status", t, c, 10)
end)

test("perf: one thread's status change, the Board alone", function()
	local app = open("board")
	local t, c = changes(app, "perf_status", 10)
	report("board status", t, c, 10)
	-- Only the card of the thread that changed works out its line again
	-- (a hundred cards on the board; each did, four times over, before).
	expect((c["board.word"] or 0) / 10 <= 2).to_be(true)
end)

test("perf: one thread's status change, Progress alone", function()
	local app = open("progress")
	-- (Every unit shown, not the first few.)
	app:click("#control-progress-more")
	app:frames(30)
	local t, c = changes(app, "perf_status", 10)
	report("progress status", t, c, 10)
	-- Only the unit of the thread that changed draws again (some fifty
	-- units; each read its clocks five times over, before).
	expect((c["progress.clocks"] or 0) / 10 <= 8).to_be(true)
	local t2, c2 = changes(app, "perf_progress", 10)
	report("progress step", t2, c2, 10)
	expect((c2["progress.clocks"] or 0) / 10 <= 12).to_be(true)
end)

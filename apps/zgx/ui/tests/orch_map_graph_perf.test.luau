-- The first Graph (ds/orch_map_graph) on a big fleet (ui/orch_perf_fixture:
-- 100 threads): it settles out of sight a few steps a frame and is drawn
-- once still, each frame well inside a script call's 50 ms, and nothing of
-- it runs off screen. Prints its numbers (`GRAPHPERF …`).

local function open(): any
	local app = mount("orch_perf_fixture.luau")
	app:resize(1440, 1400)
	app:command("perf_only_map")
	app:command("perf_big")
	app:frames(30)
	return app
end

local function frames(app: any, n: number, each: ((i: number) -> ())?): (number, number)
	local total, worst = 0, 0
	for i = 1, n do
		local t0 = os.clock()
		if each then
			each(i)
		end
		app:frames(1)
		local dt = (os.clock() - t0) * 1000
		total += dt
		worst = math.max(worst, dt)
	end
	return total, worst
end

local function counts(app: any): { [string]: number }
	app:command("perf_read")
	return app:state("perf_counts") or {}
end

test("graph perf: a big web settles without a script error, drawn a handful of times", function()
	local app = open()
	app:command("perf_reset")
	app:command("map_graph")
	local total, worst = frames(app, 300)
	local tr = app:state("control_map_graph_settle")
	local c = counts(app)
	print(`GRAPHPERF settle 300f total={math.floor(total)}ms worst={math.floor(worst)}ms settle_frames={tr.frames} runs={tr.runs} layouts={c["graph.layout"] or 0} draws={c["graph.draw"] or 0}`)
	expect(app:error()).to_be(nil)
	expect(tr.done).to_be(true)
	-- (Laid out once a settling, not once a step: a few runs, a few layouts.)
	expect((c["graph.layout"] or 0) <= 12).to_be(true)
	expect(app:count("#control-widget-map >> .orch-graph-node") > 100).to_be(true)
end)

test("graph perf: off screen, nothing runs; back, it catches up", function()
	local app = open()
	app:command("map_graph")
	app:frames(300)
	local runs = app:state("control_map_graph_settle").runs
	-- (The window too short to show the Map: it's scrolled out of view.)
	app:resize(1440, 120)
	app:frames(30)
	app:command("perf_reset")
	local total = frames(app, 600, function(i: number)
		if i % 150 == 0 then
			app:command("perf_status")
		end
	end)
	local c = counts(app)
	print(`GRAPHPERF off-screen 10s, 4 status changes: total={math.floor(total)}ms layouts={c["graph.layout"] or 0} draws={c["graph.draw"] or 0}`)
	expect(c["graph.draw"] or 0).to_be(0)
	expect(c["graph.layout"] or 0).to_be(0)
	expect(app:state("control_map_graph_settle").runs).to_be(runs)
	-- Back on screen: drawn again, the web as it is now.
	app:resize(1440, 1400)
	app:frames(120)
	c = counts(app)
	expect((c["graph.draw"] or 0) > 0).to_be(true)
	expect(app:state("control_map_graph_settle").done).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("graph perf: hidden mid-settling, it waits; shown, it finishes", function()
	local app = open()
	app:command("map_graph")
	app:frames(2)
	app:resize(1440, 120)
	app:frames(300)
	local tr = app:state("control_map_graph_settle")
	expect(tr.done).to_be(false)
	local f = tr.frames
	app:frames(300)
	expect(app:state("control_map_graph_settle").frames).to_be(f)
	app:resize(1440, 1400)
	app:frames(300)
	expect(app:state("control_map_graph_settle").done).to_be(true)
end)

test("graph perf: a Map kept on the first Graph opens on the Topology, once", function()
	-- (As a Map left on the Graph before this: kept so, not yet moved.)
	local before = mount("orch_map_fixture.luau")
	before:frames(5)
	before:set_state("control_map", { variant = "graph", window = 15 })
	before:set_state("control_map_left_graph", false)
	before:frames(2)
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:frames(10)
	expect(app:state("control_map").variant).to_be("topology")
	app:command("map_graph")
	app:frames(10)
	expect(app:state("control_map").variant).to_be("graph")
	-- (Chosen again after the move: kept at the next launch.)
	local again = mount("orch_map_fixture.luau")
	again:resize(1440, 2400)
	again:frames(10)
	expect(again:state("control_map").variant).to_be("graph")
end)

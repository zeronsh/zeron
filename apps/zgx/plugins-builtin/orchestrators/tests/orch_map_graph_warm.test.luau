-- The first Graph (ds/orch_map_graph) opening where it last settled: a
-- fresh open whose nodes were all seen is settled at once, no settling to
-- watch. Prints `GRAPHWARM …` (cold, then warm, on 164 agents).

local function launch(): any
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:command("map_only")
	app:command("map_force_big")
	app:command("map_graph")
	return app
end

-- Frames until the Graph has settled (or `most`): their count, the worst.
local function settle(app: any, most: number): (number, number, number)
	local worst, first = 0, 0
	for f = 1, most do
		local t0 = os.clock()
		app:frames(1)
		local dt = (os.clock() - t0) * 1000
		if f == 1 then
			first = dt
		end
		worst = math.max(worst, dt)
		local tr = app:state("control_map_graph_settle") or {}
		if tr.done then
			return f, worst, first
		end
	end
	return most, worst, first
end

test("graph warm: a fresh open with every node seen is settled at once", function()
	local app = launch()
	local f1, w1, a1 = settle(app, 600)
	print(`zeron-zgx: GRAPHWARM old Graph cold open 164 agents: frames_to_settled={f1} (~{math.floor(f1 * 16.7)}ms) worst={string.format("%.1f", w1)}ms first={string.format("%.1f", a1)}ms`)
	expect(f1 < 600).to_be(true)
	-- Away and back: the Graph opens afresh, every node's place kept.
	app:command("map_topology")
	app:frames(5)
	app:command("map_graph")
	local f2, w2, a2 = settle(app, 600)
	print(`zeron-zgx: GRAPHWARM old Graph warm open 164 agents: frames_to_settled={f2} (~{math.floor(f2 * 16.7)}ms) worst={string.format("%.1f", w2)}ms first={string.format("%.1f", a2)}ms`)
	expect(f2 <= 6).to_be(true)
end)

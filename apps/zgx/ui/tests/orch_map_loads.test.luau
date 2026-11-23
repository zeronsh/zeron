-- The machines' loads tick (their CPU, every few seconds) in every Map
-- view: only the headings follow it (ds/orch_map's shaped world), each tick
-- frame a fraction of a millisecond; a view kept on the grid Graph (gone)
-- opens on the force Graph. Prints `VIEWLOADS …`.

local function pct(list: { number }, p: number): number
	local s = table.clone(list)
	table.sort(s)
	return s[math.clamp(math.ceil(#s * p), 1, #s)]
end

test("map: a view kept on the grid Graph opens on the force Graph", function()
	-- (A Map left on the grid Graph before it went: kept so.)
	local before = mount("orch_map_fixture.luau")
	before:frames(5)
	before:set_state("control_map", { variant = "graph2", window = 60 })
	before:set_state("control_map_left_graph", true)
	before:frames(2)
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:command("map_only")
	app:frames(30)
	expect(app:has("#orch-force-field")).to_be(true)
end)

-- Every view on the loads tick (the machines' CPU, every few seconds):
-- only the headings follow it (ds/orch_map's shaped world), each tick
-- frame a fraction of a millisecond. Prints `VIEWLOADS …`.
test("map loads tick: every view, the headings alone", function()
	for _, v in { "topology", "timeline", "machines", "force" } do
		local app = mount("orch_map_fixture.luau")
		app:resize(1440, 1400)
		app:command("map_only")
		app:command("map_big")
		app:command(`map_{v}`)
		app:frames(60)
		local ticks = {}
		for _ = 1, 20 do
			local t0 = os.clock()
			app:command("map_big_loads")
			app:frames(1)
			table.insert(ticks, (os.clock() - t0) * 1000)
		end
		local t0 = os.clock()
		app:command(`map_topology`)
		app:frames(1)
		app:command(`map_{v}`)
		local opens = {}
		for _ = 1, 30 do
			local t1 = os.clock()
			app:frames(1)
			table.insert(opens, (os.clock() - t1) * 1000)
		end
		expect(pct(ticks, 0.5) < 4).to_be(true)
		print(`zeron-zgx: VIEWLOADS {v} tick p50={string.format("%.2f", pct(ticks, 0.5))} p95={string.format("%.2f", pct(ticks, 0.95))} max={string.format("%.2f", pct(ticks, 1))} | open worst-frame={string.format("%.1f", pct(opens, 1))} total30={string.format("%.1f", (os.clock() - t0) * 1000)}`)
	end
end)

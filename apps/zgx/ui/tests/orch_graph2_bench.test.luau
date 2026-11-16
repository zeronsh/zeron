-- The new Graph against the first on the same big fleet (100 threads, the
-- VPN orchestrator with 51 crew), headless: what a frame costs while the
-- view pans or zooms, and what a change costs. Prints its numbers
-- (`GRAPHBENCH …`); asserts only the new Graph's own guard.

local function pct(list: { number }, p: number): number
	local s = table.clone(list)
	table.sort(s)
	return s[math.clamp(math.ceil(#s * p), 1, #s)]
end

local function open(variant: string, world: string?): any
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 1400)
	app:command("map_only")
	app:command(world or "map_big")
	app:command(`map_{variant}`)
	-- (The first Graph settles: let it.)
	app:frames(if variant == "graph" then 240 else 20)
	return app
end

local function timed(app: any, n: number, step: (i: number) -> ()): { number }
	local out = {}
	for i = 1, n do
		local t0 = os.clock()
		step(i)
		app:frames(1)
		table.insert(out, (os.clock() - t0) * 1000)
	end
	return out
end

local function report(name: string, list: { number })
	print(`GRAPHBENCH {name} p50={string.format("%.2f", pct(list, 0.5))}ms p95={string.format("%.2f", pct(list, 0.95))}ms max={string.format("%.2f", pct(list, 1))}ms n={#list}`)
end

local function bench(world: string?)
	for _, v in { "graph", "graph2" } do
		local app = open(v, world)
		print(`GRAPHBENCH world={world or "map_big"} nodes={app:count("#control-widget-map >> .orch-graph2-node")}`)
		local idle = timed(app, 120, function() end)
		report(`{v} idle-frame`, idle)
		local zoom = timed(app, 120, function(i: number)
			app:command(if i % 2 == 0 then `map_{v}_zoom_in` else `map_{v}_zoom_out`)
		end)
		report(`{v} zoom-frame`, zoom)
		if v == "graph2" then
			local pan = timed(app, 120, function()
				app:command("map_graph2_pan")
			end)
			report(`{v} pan-frame`, pan)
			-- The guard: a pan or zoom frame's median far inside 16 ms, its
			-- tail inside a script call's 50 ms however busy the machine.
			expect(pct(pan, 0.5) < 4).to_be(true)
			expect(pct(zoom, 0.5) < 4).to_be(true)
			expect(pct(pan, 0.95) < 50).to_be(true)
			expect(pct(zoom, 0.95) < 50).to_be(true)
		else
			-- (The first Graph has no pan command: a drag across its field.)
			local drag = {}
			for _ = 1, 20 do
				local t0 = os.clock()
				app:drag("#orch-graph-field", "#orch-graph-fit")
				app:frames(1)
				table.insert(drag, (os.clock() - t0) * 1000)
			end
			report(`{v} drag-gesture`, drag)
		end
		-- A change of what's shown: one thread more, and the frames after.
		local t0 = os.clock()
		app:command("map_big_more")
		local change = timed(app, 60, function() end)
		report(`{v} change-frames`, change)
		print(`GRAPHBENCH {v} change-total-60f={string.format("%.1f", (os.clock() - t0) * 1000)}ms`)
		if v == "graph2" then
			print(`GRAPHBENCH graph2 layout_ms={string.format("%.3f", app:state("control_map_graph2_stats").layout_ms)}`)
		end
	end
end

test("graph2 bench: pan and zoom frames, old and new", function()
	bench()
end)

-- (On a copy of the live data, where one was put beside the fixture.)
test("graph2 bench: the live data's copy", function()
	bench("map_snapshot")
end)

-- Dragging a node of the force Graph: what a frame of it costs, on the
-- sample world (~25 agents) and the big fleet (164). Prints `FORCEDRAG …`.

local function pct(list: { number }, p: number): number
	local s = table.clone(list)
	table.sort(s)
	return s[math.clamp(math.ceil(#s * p), 1, #s)]
end

local function run(world: string?, what: string, label: string): (number, number)
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:command("map_only")
	if world then
		app:command(world)
	end
	app:command("map_force")
	app:frames(90)
	local a = app:state("control_map_force_stats") or {}
	local times = {}
	for _ = 1, 60 do
		local t0 = os.clock()
		app:command(`map_force_drag_{what}_step`)
		app:frames(1)
		table.insert(times, (os.clock() - t0) * 1000)
	end
	app:command(`map_force_drag_{what}_end`)
	local t1 = os.clock()
	local f = 0
	for i = 1, 120 do
		app:frames(1)
		f = i
		local s = app:state("control_map_force_stats") or {}
		if (s.settled or 0) > (a.settled or 0) then
			break
		end
	end
	local b = app:state("control_map_force_stats") or {}
	print(`FORCEDRAG {label}: frame p50={string.format("%.2f", pct(times, 0.5))}ms p95={string.format("%.2f", pct(times, 0.95))}ms max={string.format("%.2f", pct(times, 1))}ms base_draws={(b.draws or 0) - (a.draws or 0)} overlay={(b.overlay or 0) - (a.overlay or 0)} steps={(b.steps or 0) - (a.steps or 0)} settle_after_release={f}f/{string.format("%.1f", (os.clock() - t1) * 1000)}ms`)
	return pct(times, 0.5), pct(times, 0.95)
end

test("force drag: frames of a drag stay cheap", function()
	run(nil, "brainhub", "sample ~25 agents, an orchestrator")
	run("map_force_big", "chat", "164 agents, a chat")
	run("map_force_big", "brain", "164 agents, the Brain (the whole web following)")
	local p50 = run("map_force_big", "hub", "164 agents, a hub with 72 crew")
	expect(p50 < 16).to_be(true)
end)

local function open_big(): any
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:command("map_only")
	app:command("map_force_big")
	app:command("map_force")
	app:frames(90)
	return app
end
local function places(app: any): { [string]: { number } }
	app:command("map_force_read")
	return app:state("force_places") or {}
end

test("force drag: let go early, nothing resets; the dropped node stays, the rest where they were", function()
	local app = open_big()
	local before = places(app)
	for _ = 1, 10 do
		app:command("map_force_drag_chat_step")
		app:frames(1)
	end
	local dropped = places(app)["vpn3"]
	app:command("map_force_drag_chat_end")
	app:frames(60)
	local after = places(app)
	-- The dropped one where it was let go (a settling's nudge at most).
	local d = math.sqrt((after["vpn3"][1] - dropped[1]) ^ 2 + (after["vpn3"][2] - dropped[2]) ^ 2)
	expect(d < 12).to_be(true)
	-- (Moved from where it was before: no reset.)
	expect(math.abs(after["vpn3"][1] - before["vpn3"][1]) > 20).to_be(true)
	-- Every other hub and the Brain where they were.
	for _, id in { "orc-brain", "orc-ui", "orc-eng", "orc-vpn" } do
		if before[id] then
			expect(math.abs(after[id][1] - before[id][1]) < 0.5).to_be(true)
			expect(math.abs(after[id][2] - before[id][2]) < 0.5).to_be(true)
		end
	end
	-- And through a change of what's shown elsewhere.
	app:command("map_big_more")
	app:frames(60)
	local later = places(app)
	expect(math.abs(later["vpn3"][1] - after["vpn3"][1]) < 2).to_be(true)
	expect(math.abs(later["vpn3"][2] - after["vpn3"][2]) < 2).to_be(true)
end)

test("force drag: held still, it stays held (dragging); the mode never changes mid-drag", function()
	local app = open_big()
	app:command("map_force_drag_hub_hold")
	app:frames(2)
	expect(app:state("control_map_force_held")).to_be("orc-eng")
	local a = app:state("control_map_force_stats")
	-- (Held, unmoved: still held, the rest not drawn again.)
	for _ = 1, 30 do
		app:frames(1)
		expect(app:state("control_map_force_held")).to_be("orc-eng")
	end
	for _ = 1, 30 do
		app:command("map_force_drag_hub_step")
		app:frames(1)
		expect(app:state("control_map_force_held")).to_be("orc-eng")
	end
	local b = app:state("control_map_force_stats")
	expect(b.draws - a.draws <= 2).to_be(true)
	expect(b.settled).to_be(a.settled)
	app:command("map_force_drag_hub_end")
	app:frames(2)
	expect(app:state("control_map_force_held")).to_be(nil)
	expect(app:state("control_map_force_stats").settled).to_be(b.settled + 1)
end)

test("force graph: panned part out of view and back, zoomed, a node dragged across a band's edge: every band draws all the view reaches", function()
	local app = open_big()
	local function same_bands()
		local s = app:state("control_map_force_stats")
		local n0 = s.drawn_0
		expect(n0 ~= nil).to_be(true)
		local k = 1
		while s[`drawn_{k}`] ~= nil do
			expect(s[`drawn_{k}`]).to_be(n0)
			k += 1
		end
		expect(k >= 2).to_be(true)
	end
	same_bands()
	for _, z in { "in", "in", "out", "out", "out" } do
		app:command(`map_force_zoom_{z}`)
		for _ = 1, 12 do
			app:command("map_force_pan")
			app:frames(1)
		end
		app:frames(2)
		same_bands()
	end
	-- A hub dragged down across the band's edge (512 points), let go.
	for _ = 1, 40 do
		app:command("map_force_drag_hub_step")
		app:frames(1)
	end
	app:command("map_force_drag_hub_end")
	app:frames(5)
	same_bands()
	-- Back in view (Fit): all of it drawn, in every band.
	app:click("#orch-force-fit")
	app:frames(5)
	same_bands()
	expect(app:state("control_map_force_stats").drawn_0 >= 160).to_be(true)
end)

test("force graph: a live push changing one chat's state repaints its rings alone", function()
	local app = open_big()
	local times = {}
	local a = app:state("control_map_force_stats")
	for _ = 1, 20 do
		local t0 = os.clock()
		app:command("map_force_push")
		app:frames(1)
		table.insert(times, (os.clock() - t0) * 1000)
		app:frames(5)
	end
	local b = app:state("control_map_force_stats")
	table.sort(times)
	print(`FORCEPUSH 164 agents, one chat's state a push: frame p50={string.format("%.2f", times[10])}ms max={string.format("%.2f", times[20])}ms base_draws={b.draws - a.draws} status_draws={(b.status or 0) - (a.status or 0)} builds={b.builds - a.builds}`)
	expect(b.builds).to_be(a.builds)
end)

test("force drag: the Brain drags too, its hubs along; it stays, through a change; Fit puts it back", function()
	local app = open_big()
	local before = places(app)
	for _ = 1, 15 do
		app:command("map_force_drag_brain_step")
		app:frames(1)
	end
	app:command("map_force_drag_brain_end")
	app:frames(30)
	local after = places(app)
	local bx, by = after["orc-brain"][1], after["orc-brain"][2]
	expect(math.abs(bx - (before["orc-brain"][1] + 60)) < 1).to_be(true)
	expect(math.abs(by - (before["orc-brain"][2] + 30)) < 1).to_be(true)
	-- (Its hubs pulled along: each about as far from it as before.)
	for _, id in { "orc-ui", "orc-eng", "orc-vpn" } do
		local d0 = math.sqrt((before[id][1] - before["orc-brain"][1]) ^ 2 + (before[id][2] - before["orc-brain"][2]) ^ 2)
		local d1 = math.sqrt((after[id][1] - bx) ^ 2 + (after[id][2] - by) ^ 2)
		expect(math.abs(d1 - d0) < d0 * 0.25).to_be(true)
	end
	-- Through a change of what's shown: still there.
	app:command("map_big_more")
	app:frames(60)
	local later = places(app)
	expect(math.abs(later["orc-brain"][1] - bx) < 0.5).to_be(true)
	expect(math.abs(later["orc-brain"][2] - by) < 0.5).to_be(true)
	-- Fit: back in the centre, the web the same shape round it.
	app:click("#orch-force-fit")
	app:frames(3)
	local fit = places(app)
	expect(fit["orc-brain"][1]).to_be(0)
	expect(fit["orc-brain"][2]).to_be(0)
	expect(math.abs((fit["orc-eng"][1] - fit["orc-brain"][1]) - (later["orc-eng"][1] - later["orc-brain"][1])) < 0.5).to_be(true)
end)

-- Pins: what's dropped by hand stays; its parent moving or not.
local function drag_steps(app: any, what: string, n: number)
	for _ = 1, n do
		app:command(`map_force_drag_{what}_step`)
		app:frames(1)
	end
	app:command(`map_force_drag_{what}_end`)
	app:frames(20)
end

test("force pins: a dropped child stays when its parent is dragged; the unpinned follow", function()
	local app = open_big()
	drag_steps(app, "engchat", 8)
	expect(app:state("control_map_force_pins")["eng2"]).to_be(true)
	local before = places(app)
	drag_steps(app, "hub", 15)
	local after = places(app)
	-- The pinned one where it was dropped.
	expect(math.abs(after["eng2"][1] - before["eng2"][1]) < 0.5).to_be(true)
	expect(math.abs(after["eng2"][2] - before["eng2"][2]) < 0.5).to_be(true)
	-- An unpinned sibling followed (its offset from the hub about the same).
	local function off(p: any, id: string): (number, number)
		return p[id][1] - p["orc-eng"][1], p[id][2] - p["orc-eng"][2]
	end
	local bx, by = off(before, "eng3")
	local ax, ay = off(after, "eng3")
	expect(math.sqrt((ax - bx) ^ 2 + (ay - by) ^ 2) < 25).to_be(true)
	expect(math.abs(after["eng3"][1] - before["eng3"][1]) > 20).to_be(true)
end)

test("force pins: Unpin all children lets go of that parent's only; Unpin all of every one", function()
	local app = open_big()
	drag_steps(app, "engchat", 8)
	drag_steps(app, "chat", 8)
	local pins = app:state("control_map_force_pins")
	expect(pins["eng2"]).to_be(true)
	expect(pins["vpn3"]).to_be(true)
	app:command("map_force_unpin_children")
	app:frames(60)
	pins = app:state("control_map_force_pins")
	expect(pins["eng2"]).to_be(nil)
	expect(pins["vpn3"]).to_be(true)
	app:command("map_force_unpin_all")
	app:frames(60)
	expect((next(app:state("control_map_force_pins")))).to_be(nil)
end)

test("force pins: kept through a change of what's shown, and a relaunch", function()
	local app = open_big()
	drag_steps(app, "engchat", 8)
	local at = places(app)["eng2"]
	app:command("map_big_more")
	app:frames(60)
	local later = places(app)["eng2"]
	expect(math.abs(later[1] - at[1]) < 0.5).to_be(true)
	local again = open_big()
	expect(again:state("control_map_force_pins")["eng2"]).to_be(true)
	local relaunched = places(again)["eng2"]
	expect(math.abs(relaunched[1] - at[1]) < 0.5).to_be(true)
	expect(math.abs(relaunched[2] - at[2]) < 0.5).to_be(true)
	-- (Fit is the view's alone: the pin stays.)
	again:click("#orch-force-fit")
	again:frames(5)
	expect(again:state("control_map_force_pins")["eng2"]).to_be(true)
end)

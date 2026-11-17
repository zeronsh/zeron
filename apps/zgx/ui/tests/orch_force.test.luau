-- The force Graph (ds/orch_force, ds/orch_force_sim, ds/orch_force_model):
-- the same seed and nodes settle the same; it opens where it last settled
-- and holds still; it stops when still and does nothing idle or off
-- screen; its canvases stay under zgx's raster cap; on 160 threads it
-- opens and settles fast, no frame long. Prints `FORCEPERF …`.

local sim = require("@ds/orch_force_sim")

local function open(w: number?, fleet: string?): any
	local app = mount("orch_map_fixture.luau")
	app:resize(w or 1440, 2400)
	app:command("map_only")
	app:command(fleet or "map_force_big")
	app:command("map_force")
	return app
end

local function stats(app: any): any
	return app:state("control_map_force_stats") or {}
end

-- Frames until settled (or `most`): how many, the worst's wall time.
local function settle(app: any, most: number): (number, number)
	local worst = 0
	for f = 1, most do
		local t0 = os.clock()
		app:frames(1)
		worst = math.max(worst, (os.clock() - t0) * 1000)
		local s = stats(app)
		if (s.settled or 0) > 0 and (s.wakes or 0) > 0 and s.settled >= 1 and app:has(".orch-force-node") then
			return f, worst
		end
	end
	return most, worst
end

-- A web of a hub with `n` round it, and a ring of others.
local function web(n: number): ({ sim.Node }, { sim.Link })
	local nodes: { sim.Node } = { { id = "brain", fixed = true, mass = 4 } }
	local links: { sim.Link } = {}
	for k = 1, 4 do
		table.insert(nodes, { id = `o{k}`, parent = 1, mass = 2.5 })
		table.insert(links, { a = 1, b = #nodes, strong = true, hub = true })
	end
	for k = 1, n do
		local p = 2 + (k % 4)
		table.insert(nodes, { id = `c{k}`, parent = p })
		table.insert(links, { a = p, b = #nodes, strong = true, hub = false })
	end
	for k = 1, n // 10 do
		table.insert(links, { a = 5 + k, b = 5 + n - k, strong = false, hub = false })
	end
	return nodes, links
end

test("force sim: the same seed and web settle to the same places", function()
	local function settled(): sim.Sim
		local nodes, links = web(60)
		local s = sim.new(nodes, links, 11)
		sim.place(s, nil)
		sim.wake(s)
		for _ = 1, 2000 do
			sim.step(s)
			if sim.still(s) then
				break
			end
		end
		return s
	end
	local a, b = settled(), settled()
	expect(a.steps).to_be(b.steps)
	for i = 1, a.n do
		expect(a.x[i]).to_be(b.x[i])
		expect(a.y[i]).to_be(b.y[i])
	end
	-- (The Brain dump held in the middle.)
	expect(a.x[1]).to_be(0)
	expect(a.y[1]).to_be(0)
end)

test("force sim: it comes to rest, and a still web stays still", function()
	local nodes, links = web(150)
	local s = sim.new(nodes, links, 3)
	sim.place(s, nil)
	sim.wake(s)
	local t0 = os.clock()
	local still = false
	for _ = 1, 100 do
		if sim.run(s, 0.005, os.clock) then
			still = true
			break
		end
	end
	local ms = (os.clock() - t0) * 1000
	print(`FORCEPERF sim 154 nodes cold: steps={s.steps} ms={string.format("%.1f", ms)}`)
	expect(still).to_be(true)
	-- From where it rested: nothing new, nothing moves.
	local kept = sim.positions(s)
	local s2 = sim.new(nodes, links, 3)
	local fresh = sim.place(s2, kept)
	expect(fresh).to_be(0)
	sim.wake(s2)
	sim.step(s2)
	expect(s2.energy < 2).to_be(true)
end)

test("force sim: a new node starts by its parent; waking it wakes its neighbours only", function()
	local nodes, links = web(40)
	local s = sim.new(nodes, links, 5)
	sim.place(s, nil)
	sim.wake(s)
	for _ = 1, 2000 do
		sim.step(s)
		if sim.still(s) then
			break
		end
	end
	local kept = sim.positions(s)
	table.insert(nodes, { id = "new", parent = 2 })
	table.insert(links, { a = 2, b = #nodes, strong = true, hub = false })
	local s2 = sim.new(nodes, links, 5)
	expect(sim.place(s2, kept)).to_be(1)
	local i = s2.index["new"]
	local d = math.sqrt((s2.x[i] - s2.x[2]) ^ 2 + (s2.y[i] - s2.y[2]) ^ 2)
	expect(math.abs(d - s2.slot_r[i]) < 3).to_be(true)
	sim.wake(s2, { i })
	expect(s2.awake[i]).to_be(true)
	expect(s2.awake[2]).to_be(true)
	expect(s2.awake[1]).to_be(false)
	expect(s2.awake[10]).to_be(false)
end)

test("force graph: a cold open settles a frame at a time, no frame long, then nothing runs", function()
	local app = open()
	local f, worst = settle(app, 240)
	local s = stats(app)
	print(`FORCEPERF cold open 164 agents: frames_to_settled={f} (~{math.floor(f * 16.7)}ms) worst_frame={string.format("%.1f", worst)}ms sim_worst={string.format("%.1f", s.worst_ms or 0)}ms first_draw={string.format("%.1f", s.first_ms or 0)}ms steps={s.steps} nodes={s.nodes}`)
	expect(s.nodes >= 160).to_be(true)
	expect((s.settled or 0) >= 1).to_be(true)
	expect(f <= 60).to_be(true)
	expect(app:error() == nil or not string.find(app:error(), "orch_force", 1, true)).to_be(true)
	-- Idle: nothing drawn, no step; the working rings' overlay steps ten
	-- times a second at most.
	local a = stats(app)
	app:frames(120)
	local b = stats(app)
	expect(b.draws).to_be(a.draws)
	expect(b.steps).to_be(a.steps)
	expect(b.overlay - a.overlay <= 2 * 21).to_be(true)
end)

test("force graph: settled, the overlay draws nothing that moved (its last settling frame never stays over the drawing)", function()
	local app = open()
	settle(app, 240)
	app:frames(5)
	expect(stats(app).over_moving).to_be(0)
	-- (After a drag let go too.)
	app:command("map_force_drag_hub_step")
	app:frames(3)
	expect(stats(app).over_moving > 0).to_be(true)
	app:command("map_force_drag_hub_end")
	app:frames(30)
	expect(stats(app).over_moving).to_be(0)
end)

test("force graph: a warm open (places kept) is settled on its first frame", function()
	-- (A first launch: the sample world, then the big fleet, each settled
	-- and kept.)
	local function launch(): any
		local app = mount("orch_map_fixture.luau")
		app:resize(1440, 2400)
		app:command("map_only")
		app:command("map_force")
		return app
	end
	local first = launch()
	settle(first, 240)
	first:command("map_force_big")
	settle(first, 240)
	-- The next launch: both open where they rested, nothing to settle.
	local app = launch()
	app:frames(3)
	local a = stats(app)
	local t0 = os.clock()
	app:command("map_force_big")
	app:frames(1)
	local ms = (os.clock() - t0) * 1000
	app:frames(2)
	local s = stats(app)
	print(`FORCEPERF warm open 164 agents: first frame={string.format("%.1f", ms)}ms wakes={s.wakes} steps={s.steps} first_draw={string.format("%.1f", s.first_ms or 0)}ms`)
	expect(s.wakes or 0).to_be(0)
	expect(s.steps or 0).to_be(0)
	expect(s.builds > (a.builds or 0)).to_be(true)
	expect(ms < 50).to_be(true)
	expect(app:count("#control-widget-map >> .orch-force-node") >= 160).to_be(true)
end)

test("force graph: a pan or a zoom moves the drawing as it stands", function()
	local app = open()
	settle(app, 240)
	local a = stats(app)
	for _ = 1, 4 do
		app:command("map_force_pan")
		app:frames(1)
	end
	app:command("map_force_zoom_in")
	app:frames(2)
	local b = stats(app)
	expect(b.steps).to_be(a.steps)
	expect(b.builds).to_be(a.builds)
	expect(b.draws - a.draws <= 2).to_be(true)
end)

test("force graph: a dragged hub pulls its own, the rest stay; then still", function()
	local app = open()
	settle(app, 240)
	local a = stats(app)
	app:command("map_force_drag")
	local f = settle(app, 240)
	local b = stats(app)
	expect(b.wakes).to_be(a.wakes + 1)
	expect(b.steps > a.steps).to_be(true)
	expect(b.settled).to_be(a.settled + 1)
	expect(f < 240).to_be(true)
end)

test("force graph: a new thread wakes only round it, and settles quickly", function()
	local app = open()
	settle(app, 240)
	local a = stats(app)
	app:command("map_big_more")
	local t0 = os.clock()
	local f, worst = settle(app, 240)
	local b = stats(app)
	print(`FORCEPERF one more thread: frames={f} worst={string.format("%.1f", worst)}ms steps={b.steps - a.steps} total={string.format("%.1f", (os.clock() - t0) * 1000)}ms`)
	expect(b.builds).to_be(a.builds + 1)
	expect(f <= 30).to_be(true)
end)

test("force graph: off screen nothing settles, draws or builds; back, it takes up where it was", function()
	local app = open()
	app:frames(3)
	-- (The window too short to show the Map: it's out of view.)
	app:resize(1440, 40)
	app:frames(30)
	local a = stats(app)
	for i = 1, 300 do
		if i == 100 then
			app:command("map_big_more")
		end
		app:frames(1)
	end
	local b = stats(app)
	expect(b.steps).to_be(a.steps)
	expect(b.draws).to_be(a.draws)
	expect(b.builds).to_be(a.builds)
	app:resize(1440, 2400)
	local f = settle(app, 300)
	expect(f < 300).to_be(true)
end)

test("force graph: its canvases stay under zgx's raster cap at any width", function()
	for _, w in { 900, 1440, 2560 } do
		local app = open(w)
		app:frames(5)
		for line in app:tree("#orch-force-field"):gmatch("[^\n]+") do
			local cw, ch = line:match("orch%-force%-canvas.- ([%d%.]+)x([%d%.]+)")
			if not cw then
				cw, ch = line:match("%.orch%-force%-canvas.- ([%d%.]+)x([%d%.]+)")
			end
			if cw then
				-- (At a 3x display: 4 bytes a pixel.)
				expect((tonumber(cw) :: number) * 3 * (tonumber(ch) :: number) * 3 * 4 < 32 * 1024 * 1024).to_be(true)
			end
		end
		expect(app:count(".orch-force-canvas") >= 1).to_be(true)
	end
end)

test("force graph: filled by machine, a legend names them; no Colour by; a theme change redraws, lays out nothing", function()
	local app = open()
	settle(app, 240)
	-- (The legend: a machine each, those with a node on the web.)
	expect(app:count("#control-widget-map >> .orch-force-legend-item") >= 1).to_be(true)
	app:click('#control-widget-map >> role:button "Widget menu"')
	app:frames(3)
	expect(app:has('role:menuitemcheckbox "Machine"')).to_be(false)
	expect(app:has('role:menuitemcheckbox "Orchestrator"')).to_be(false)
	app:key("Escape")
	app:frames(3)
	local a = stats(app)
	app:command("theme_light")
	app:frames(3)
	-- (Drawn again in the theme's colours; nothing laid out or settled.)
	local b = stats(app)
	expect(b.draws > a.draws).to_be(true)
	expect(b.steps).to_be(a.steps)
	expect(b.builds).to_be(a.builds)
	app:command("theme_dark")
	app:frames(3)
end)

test("force graph: the Brain, orchestrators and those asking are named; working chats wear the sidebar's ring, those asking its glyph; idle, nothing draws", function()
	local app = open()
	settle(app, 240)
	expect(app:has('#control-widget-map >> .orch-force-brain "Brain"') or app:has('#control-widget-map >> .orch-force-brain')).to_be(true)
	-- (The sidebar's own StatusIcon over the canvas: a ring a working chat,
	-- a glyph an asking, approving or failed one.)
	expect(app:count("#control-widget-map >> .orch-force-ring") >= 1).to_be(true)
	expect(app:count("#control-widget-map >> .orch-force-glyph") >= 1).to_be(true)
	-- (Idle: the canvases stand as they were.)
	local a = stats(app)
	app:frames(30)
	local b = stats(app)
	expect(b.draws).to_be(a.draws)
	expect(b.status).to_be(a.status)
	expect(b.steps).to_be(a.steps)
end)

test("force graph: hover, card, grip, click, menu", function()
	local app = open()
	settle(app, 240)
	app:set_state("control_map_force_hover", "vpn19")
	app:frames(3)
	app:hover(".orch-force-anchor-box")
	expect(app:wait(".orch-force-card", { timeout_ms = 1500 })).to_be(true)
	app:click(".orch-force-anchor-box")
	app:frames(5)
	expect(app:state("shell_route_path")).to_be("/session/vpn19")
	-- A secondary click's menu.
	local m = mount("orch_map_fixture.luau")
	m:resize(1440, 2400)
	m:frames(20)
	m:command("map_force")
	m:frames(60)
	m:set_state("map_menu_id", "orc-orch")
	m:command("map_menu")
	m:frames(10)
	expect(m:has('"Pause"')).to_be(true)
end)

test("force graph: a node's menu mentions it in Capture's composer; no grip, no hover chip", function()
	local app = open()
	settle(app, 240)
	app:set_state("map_menu_id", "vpn3")
	app:command("map_menu")
	app:frames(10)
	expect(app:has('"Mention in composer"')).to_be(true)
	app:key("Escape")
	app:frames(3)
	app:set_state("map_menu_pick", "Mention in composer")
	app:command("map_menu_choose")
	app:frames(3)
	app:command("map_read_cargo")
	local cargo = app:state("brain_cargo")
	local found = false
	for _, item in cargo do
		if item.kind == "ref" and item.id == "vpn3" then
			found = true
		end
	end
	expect(found).to_be(true)
	-- An orchestrator too.
	app:set_state("map_menu_id", "orc-ui")
	app:set_state("map_menu_pick", "Mention in composer")
	app:command("map_menu_choose")
	app:frames(3)
	app:command("map_read_cargo")
	local ids = {}
	for _, item in app:state("brain_cargo") do
		ids[item.id or ""] = true
	end
	expect(ids["orc-ui"]).to_be(true)
	-- Hover: the card alone.
	app:set_state("control_map_force_hover", "vpn19")
	app:frames(3)
	app:hover(".orch-force-anchor-box")
	expect(app:wait(".orch-force-card", { timeout_ms = 1500 })).to_be(true)
	expect(app:has("#orch-force-grip")).to_be(false)
end)

-- One link a pair (ds/orch_force_model): a strong pair never gets a weak
-- one; messages either way, or many, make one weak link.
local fm = require("@ds/orch_force_model")
local function world(comms_pairs: { { string } }): (any, { any })
	local t = os.time()
	local chats = {
		["b"] = { id = "b", title = "Brain dump", status = "working", device = "d1", updated = t },
		["o"] = { id = "o", title = "Orch", status = "working", device = "d1", updated = t },
		["c1"] = { id = "c1", title = "One", status = "working", device = "d1", updated = t },
		["c2"] = { id = "c2", title = "Two", status = "working", device = "d1", updated = t },
		["c3"] = { id = "c3", title = "Three", status = "working", device = "d1", updated = t },
	}
	local now = {
		orchestrators = {
			{ id = "b", kind = "brain_dump", name = "Brain dump", hue = 140 },
			{ id = "o", kind = "standard", name = "Orch", hue = 200 },
		},
		crew = {
			{ orchestrator_id = "o", chat_id = "c1" },
			{ orchestrator_id = "o", chat_id = "c2" },
			{ orchestrator_id = "o", chat_id = "c3" },
		},
		items = {},
		asks = {},
	}
	local comms = {}
	for k, pr in comms_pairs do
		table.insert(comms, { id = `m{k}`, from = pr[1], to = pr[2], at = t - k })
	end
	return { now = now, chats = chats, devices = { { id = "d1", name = "desk", here = true } }, here = "d1", show = "all", clock = t }, comms
end
local function links_between(m: any, a: string, b: string): (number, number, number)
	local ia, ib = m.index[a], m.index[b]
	local strong, weak, arcs = 0, 0, 0
	for _, l in m.links do
		if (l.a == ia and l.b == ib) or (l.a == ib and l.b == ia) then
			if l.strong then
				strong += 1
			else
				weak += 1
			end
		end
	end
	for _, arc in m.arcs do
		if (arc.a == ia and arc.b == ib) or (arc.a == ib and arc.b == ia) then
			arcs += 1
		end
	end
	return strong, weak, arcs
end

test("force model: a strong pair with messages either way has its one strong link, no weak one, no arc", function()
	local w, comms = world({ { "o", "c1" }, { "c1", "o" }, { "c1", "o" } })
	local m = fm.build(w, comms, w.clock)
	local strong, weak, arcs = links_between(m, "o", "c1")
	expect(strong).to_be(1)
	expect(weak).to_be(0)
	expect(arcs).to_be(0)
end)

test("force model: messages both ways, and repeated, make one weak link, counted", function()
	local w, comms = world({ { "c1", "c2" }, { "c2", "c1" }, { "c1", "c2" }, { "c1", "c2" } })
	local m = fm.build(w, comms, w.clock)
	local strong, weak, arcs = links_between(m, "c1", "c2")
	expect(strong).to_be(0)
	expect(weak).to_be(1)
	expect(arcs).to_be(1)
	for _, l in m.links do
		if not l.strong then
			expect(l.count).to_be(4)
			expect((l.ab :: number) + (l.ba :: number)).to_be(4)
		end
	end
end)

-- The working ring (the sidebar's, over the canvas) round each working chat
-- exactly where its status ring is: R_RING across at the zoom, never the
-- size it was made at, whatever the zoom, through a zoom, a drag and a
-- settle, for every working chat (hubs carry no ring).
local draw = require("@ds/orch_force_draw")
local function ring_boxes(app: any): { { number } }
	local ok, err = pcall(function()
		return app:tree(".orch-force-ring")
	end)
	local out = {}
	local text = if ok then "" else tostring(err)
	for x, y, w, h in string.gmatch(text, "@%d+ (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)") do
		table.insert(out, { tonumber(x) :: number, tonumber(y) :: number, tonumber(w) :: number, tonumber(h) :: number })
	end
	return out
end
local function rings_match(app: any, label: string)
	local z = stats(app).zoom
	expect(type(z) == "number").to_be(true)
	local want = draw.R_RING * 24 / 10 * z
	local boxes = ring_boxes(app)
	expect(`{label}: {#boxes > 0}`).to_be(`{label}: true`)
	for _, b in boxes do
		-- (Its drawn ring, 10 of its 24: the status ring's R_RING, to half
		-- a point.)
		local r = b[3] * 10 / 24
		expect(`{label}: ring {string.format("%.1f", r)} status {string.format("%.1f", draw.R_RING * z)} {math.abs(r - draw.R_RING * z) <= 0.5 and math.abs(b[3] - b[4]) <= 0.5}`).to_be(`{label}: ring {string.format("%.1f", r)} status {string.format("%.1f", draw.R_RING * z)} true`)
		expect(math.abs(b[3] - want) <= 0.75).to_be(true)
	end
end

test("force graph: every working ring hugs its chat as the status rings do, at any zoom, through a zoom, a drag and a settle", function()
	local app = open()
	settle(app, 240)
	app:frames(10)
	rings_match(app, "fitted")
	for k, step in { "in", "in", "out", "out", "out", "out" } do
		app:command(`map_force_zoom_{step}`)
		app:frames(6)
		rings_match(app, `zoom {k} {step}`)
	end
	-- (A chat starting work after the zooms: made at this zoom, the same.)
	app:command("map_force_push")
	app:frames(6)
	app:command("map_force_push")
	app:frames(6)
	rings_match(app, "a chat back at work")
	-- A drag held, then let go and settled.
	app:command("map_force_drag_hub_step")
	app:frames(3)
	app:command("map_force_drag_hub_step")
	app:frames(3)
	app:command("map_force_drag_hub_end")
	app:frames(30)
	rings_match(app, "after a drag")
	app:command("map_force_zoom_in")
	app:frames(6)
	rings_match(app, "zoomed after the drag")
end)

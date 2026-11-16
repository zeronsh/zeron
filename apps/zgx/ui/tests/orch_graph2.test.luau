-- The Map's new Graph (ds/orch_graph2, ds/orch_graph2_layout): laid out
-- once per change, the same every time for the same world; a pan or zoom
-- never lays out or draws again; what's off the field isn't drawn; a change
-- eases into place; menus, cards, the archived left out, renames and
-- message dots kept; fast. On ui/orch_map_fixture.luau's big fleet (104
-- threads, the VPN orchestrator with 51 crew).

local L = require("@ds/orch_graph2_layout")
local Scene = require("@ds/orch_graph2_scene")
local model = require("@ds/orch_map_model")
local data = require("@ds/orch_data")
local sample = require("@account/orchestration_sample")

local function open(w: number?): any
	local app = mount("orch_map_fixture.luau")
	app:resize(w or 1440, 2400)
	app:command("map_only")
	app:command("map_big")
	app:command("map_graph2")
	app:frames(20)
	return app
end

-- The sample's busier world, as the page hands it to the Map.
local function world(gone: { [string]: boolean }?): any
	local f = (sample :: any).fleet
	local s = { chats = {}, children = {}, archived = {} }
	for _, c in f.chats do
		table.insert(if gone and gone[c.id] then s.archived else s.chats, c)
	end
	return { now = f.now, chats = data.chats(s), devices = f.devices, projects = f.projects, loads = f.loads, here = "dev-desk" }
end

-- A scene of `n` crew under one orchestrator, and a few own threads.
local function synthetic(n: number): any
	local members = {}
	for i = 1, n do
		table.insert(members, { id = `c{i}`, title = `Crew thread {i}`, kind = "crew", hue = 200 })
	end
	local own = {}
	for i = 1, 6 do
		table.insert(own, { id = `o{i}`, title = `Own {i}`, kind = "thread" })
	end
	return { machines = {
		{ id = "a", name = "a", groups = { { id = "x", lead = { id = "x", title = "X", kind = "orchestrator", hue = 200 }, members = members } } },
		{ id = "b", name = "b", groups = { { id = "own@b", members = own } } },
		{ id = "c", name = "c", groups = {} },
	}, links = {} }
end

test("graph2 layout: the same world and width give the same positions, every time", function()
	for _, w in { 900, 1200, 1440 } do
		local a = L.layout(Scene.of(world()), w)
		local b = L.layout(Scene.of(world()), w)
		expect(a.sig).to_be(b.sig)
		expect(#a.nodes).to_be(#b.nodes)
		for i, n in a.nodes do
			expect(b.nodes[i].id).to_be(n.id)
			expect(b.nodes[i].x).to_be(n.x)
			expect(b.nodes[i].y).to_be(n.y)
		end
	end
end)

test("graph2 layout: every node inside its machine, every machine inside the field, the field filled", function()
	for _, w in { 900, 1200, 1440 } do
		local l = L.layout(synthetic(51), w)
		local area = 0
		for _, m in l.machines do
			local r = m.rect
			expect(r.x >= 0 and r.y >= 0 and r.x + r.w <= w + 0.01 and r.y + r.h <= l.h + 0.01).to_be(true)
			area += r.w * r.h
		end
		-- (Regions cover the field but for its gutters: no empty sides.)
		expect(area / (w * l.h) > 0.85).to_be(true)
		for _, n in l.nodes do
			local r = l.machines[n.machine].rect
			expect(n.x >= r.x and n.x <= r.x + r.w and n.y >= r.y and n.y <= r.y + r.h).to_be(true)
			expect(n.cx + n.cw <= r.x + r.w + 0.01).to_be(true)
		end
	end
end)

test("graph2 layout: each crew thread is reached by an orthogonal line from its lead", function()
	local l = L.layout(synthetic(51), 1200)
	for i, n in l.nodes do
		local pts = L.path(l, i)
		if n.kind == "crew" then
			expect(pts ~= nil).to_be(true)
			local p = pts :: { number }
			expect(p[#p - 1]).to_be(n.x)
			expect(p[#p]).to_be(n.y)
			for k = 3, #p, 2 do
				expect(p[k] == p[k - 2] or p[k + 1] == p[k - 1]).to_be(true)
			end
		end
	end
end)

test("graph2 layout: a point finds its cell by arithmetic", function()
	local l = L.layout(synthetic(51), 1200)
	for i, n in l.nodes do
		expect(L.cell_at(l, n.x + 2, n.y)).to_be(i)
	end
end)

test("graph2 perf: laying out 150 threads takes well under 5 ms", function()
	local s = synthetic(150)
	local t0 = os.clock()
	for _ = 1, 20 do
		L.layout(s, 1440)
	end
	local ms = (os.clock() - t0) * 1000 / 20
	expect(ms < 5).to_be(true)
end)

test("graph2: nothing archived is in the new Graph's scene", function()
	local gone = { ["orc-vpn"] = true, ["th-engine-flow"] = true, ["th-zgx-mem"] = true }
	local l = L.layout(Scene.of(world(gone)), 1200)
	for id in gone do
		expect(l.index[id]).to_be(nil)
		expect(l.index[`ghost:{id}@dev-mac`]).to_be(nil)
	end
	expect(l.index["orc-orch"] ~= nil).to_be(true)
end)

test("graph2: shows every thread of the big fleet, filling the width", function()
	for _, w in { 900, 1200, 1440 } do
		local app = open(w)
		local s = app:state("control_map_graph2_stats")
		local line = app:tree("#orch-graph2-field"):match("[^\n]+")
		local fw = tonumber(line:match(" %-?[%d%.]+,%-?[%d%.]+ ([%d%.]+)x")) :: number
		expect(math.abs(s.w - fw) < 1).to_be(true)
		expect(s.drawn).to_be(105)
		-- 100 threads, 4 leads, 1 ghost.
		expect(app:count("#control-widget-map >> .orch-graph2-node")).to_be(105)
		expect(app:count("#control-widget-map >> .orch-graph2-machine")).to_be(4)
		expect((string.find(app:error() or "", "graph2", 1, true))).to_be(nil)
	end
end)

test("graph2: a pan or a zoom lays out nothing and draws nothing again", function()
	local app = open(1440)
	app:frames(10)
	local before = app:state("control_map_graph2_stats")
	local layouts, draws = before.layouts, before.draws
	for _ = 1, 5 do
		app:command("map_graph2_pan")
		app:frames(2)
	end
	app:command("map_graph2_zoom_in")
	app:frames(5)
	local after = app:state("control_map_graph2_stats")
	expect(after.layouts).to_be(layouts)
	expect(after.draws).to_be(draws)
end)

test("graph2: idle, nothing draws", function()
	local app = open(1440)
	app:frames(30)
	local a = app:state("control_map_graph2_stats")
	app:frames(60)
	local b = app:state("control_map_graph2_stats")
	expect(b.draws).to_be(a.draws)
	expect(b.overlay).to_be(a.overlay)
end)

test("graph2: what's far off the field isn't drawn till the view comes near", function()
	local app = open(1440)
	local a = app:state("control_map_graph2_stats")
	expect(a.drawn).to_be(105)
	-- Zoomed well in and moved along: what's drawn is the view's reach
	-- (half a field past it each way), not all of it.
	for _ = 1, 8 do
		app:command("map_graph2_zoom_in")
		app:frames(1)
	end
	local panned_draws = {}
	for _ = 1, 20 do
		app:command("map_graph2_pan")
		app:frames(1)
		table.insert(panned_draws, app:state("control_map_graph2_stats").draws)
	end
	app:frames(3)
	local b = app:state("control_map_graph2_stats")
	expect(b.draws > a.draws).to_be(true)
	expect(b.drawn < 75).to_be(true)
	-- (Drawn again now and then as the view nears the edge, never each pan.)
	expect(b.draws - a.draws < 8).to_be(true)
end)

test("graph2: a change lays out again once and eases into place", function()
	local app = open(1440)
	local a = app:state("control_map_graph2_stats")
	app:command("map_big_more")
	app:frames(2)
	local b = app:state("control_map_graph2_stats")
	expect(b.layouts).to_be(a.layouts + 1)
	-- (The ease: a few draws over its 250 ms, then still.)
	app:wait("#never-there", { timeout_ms = 400 })
	local c = app:state("control_map_graph2_stats")
	expect(c.draws > b.draws).to_be(true)
	app:frames(30)
	expect(app:state("control_map_graph2_stats").draws).to_be(c.draws)
	expect(app:count("#control-widget-map >> .orch-graph2-node")).to_be(106)
end)

test("graph2: a state changing draws again without laying out", function()
	local app = open(1440)
	local a = app:state("control_map_graph2_stats")
	app:command("map_big")
	app:frames(5)
	expect(app:state("control_map_graph2_stats").layouts).to_be(a.layouts)
end)

test("graph2: a report runs a dot along its line, then all is still", function()
	local app = open(1440)
	app:frames(10)
	local a = app:state("control_map_graph2_stats")
	app:command("map_big_report")
	app:wait("#never-there", { timeout_ms = 300 })
	local b = app:state("control_map_graph2_stats")
	expect(b.overlay > a.overlay).to_be(true)
	expect(b.draws).to_be(a.draws)
	app:wait("#never-there", { timeout_ms = 900 })
	local c = app:state("control_map_graph2_stats")
	app:frames(30)
	expect(app:state("control_map_graph2_stats").overlay).to_be(c.overlay)
end)

test("graph2: the pointer on a thread lights it and shows its card; a click opens it", function()
	local app = open(1440)
	-- (The pointer onto a thread: the field finds it by its cell.)
	app:hover('#control-widget-map >> .orch-graph2-machine "macbook"')
	app:frames(3)
	expect(app:state("control_map_graph2_hover")).to_be("dev:dev-mac")
	app:set_state("control_map_graph2_hover", "vpn19")
	app:frames(3)
	local a = app:state("control_map_graph2_stats")
	app:hover(".orch-graph2-anchor-box")
	expect(app:wait(".orch-graph2-card", { timeout_ms = 1500 })).to_be(true)
	expect(app:has("#orch-graph2-grip")).to_be(true)
	-- (Lit on the overlay alone: the rest isn't drawn again.)
	expect(app:state("control_map_graph2_stats").draws).to_be(a.draws)
	app:click(".orch-graph2-anchor-box")
	app:frames(5)
	expect(app:state("shell_route_path")).to_be("/session/vpn19")
end)

test("graph2: a click on a lead opens its orchestrator", function()
	local app = open(1440)
	app:click('#control-widget-map >> .orch-graph2-orchestrator ~"Exnomic VPN"')
	app:frames(5)
	expect(app:state("shell_route_path")).to_be("/orchestrator/orc-vpn")
end)

local function menu_for(app: any, id: string)
	app:set_state("map_menu_id", id)
	app:command("map_menu")
	app:frames(10)
end

test("graph2: a secondary click on a thread, an orchestrator or a machine opens its menu", function()
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:frames(30)
	app:command("map_graph2")
	app:frames(20)
	menu_for(app, "dev:dev-mac")
	expect(app:has('"Rename device"')).to_be(true)
	menu_for(app, "orc-orch")
	for _, item in { "Open", "Pause", "Settings…", "Archive" } do
		expect(app:has(`"{item}"`)).to_be(true)
	end
	menu_for(app, "th-ui-control")
	for _, item in { "Open", "Stop", "Release from crew", "Archive" } do
		expect(app:has(`"{item}"`)).to_be(true)
	end
end)

test("graph2: a renamed machine shows its new name at once (machines keep their order by name)", function()
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:frames(30)
	app:command("map_live")
	app:command("map_graph2")
	app:frames(20)
	local a = app:state("control_map_graph2_stats")
	app:command("fixture.rename_win")
	app:frames(5)
	expect(app:has('#control-widget-map >> .orch-graph2-machine "x16-win"')).to_be(true)
	expect(app:has('#control-widget-map >> .orch-graph2-machine "DESKTOP-3NFV6JC"')).to_be(false)
	expect(app:state("control_map_graph2_stats").layouts <= a.layouts + 1).to_be(true)
end)

test("graph2: crew are named as every view names them (ds/orch_name)", function()
	local f = (sample :: any).fleet
	local fleet = model.fleet(world())
	local l = L.layout(Scene.of(world()), 1200)
	-- (Those shown: a quiet one may be left out, ds/orch_map_visibility.)
	local named = 0
	for _, n in l.nodes do
		local t = fleet.threads[n.id]
		if t then
			expect(n.title).to_be(t.title)
			named += 1
		end
	end
	expect(named > 0).to_be(true)
	expect(f ~= nil).to_be(true)
end)

test("graph2: a dot's frames and the ease's frames allocate nothing", function()
	local app = open(1440)
	app:frames(10)
	app:command("map_big_report")
	local most = 0
	for _ = 1, 30 do
		app:frames(2)
		most = math.max(most, app:state("control_map_graph2_stats").overlay_kb)
	end
	expect(most < 1).to_be(true)
	-- (A change: the ease's frames draw marks and lines only, the names
	-- once it's done.)
	app:command("map_big_more")
	app:frames(2)
	local ease = 0
	for _ = 1, 10 do
		app:frames(1)
		ease = math.max(ease, app:state("control_map_graph2_stats").draw_kb)
	end
	print("GRAPH2ALLOC overlay_kb", most, "ease draw_kb", ease)
	expect(ease < 2).to_be(true)
end)

-- Visibility: the Map's one rule (ds/orch_map_visibility, its own tests),
-- as the new Graph follows it.

test("graph2 visibility: quiet threads leave the layout, fading ones show faint, once laid out again", function()
	local app = open(1440)
	local a = app:state("control_map_graph2_stats")
	expect(a.faded).to_be(0)
	app:command("map_big_stale")
	app:wait("#never-there", { timeout_ms = 400 })
	local b = app:state("control_map_graph2_stats")
	expect(b.layouts).to_be(a.layouts + 1)
	local shown = app:count("#control-widget-map >> .orch-graph2-node")
	expect(shown < 105).to_be(true)
	expect(b.faded > 0).to_be(true)
	expect(b.drawn).to_be(shown)
	-- "All": every crew thread back (the person's own quiet ones the Map's
	-- model leaves out itself); "Active only": the faded ones gone too.
	app:command("map_show_all")
	app:wait("#never-there", { timeout_ms = 400 })
	expect(app:count("#control-widget-map >> .orch-graph2-node") > shown).to_be(true)
	app:command("map_show_active")
	app:wait("#never-there", { timeout_ms = 400 })
	expect(app:count("#control-widget-map >> .orch-graph2-node") < shown).to_be(true)
	expect(app:state("control_map_graph2_stats").faded).to_be(0)
	app:command("map_show_recent")
	app:frames(5)
end)

test("graph2 visibility: a quiet thread at work again comes back", function()
	local app = open(1440)
	app:command("map_big_stale")
	app:wait("#never-there", { timeout_ms = 400 })
	expect(app:has('#control-widget-map >> .orch-graph2-thread ~"WireGuard keys"')).to_be(true)
	local before = app:count("#control-widget-map >> .orch-graph2-node")
	app:command("map_big_wake")
	app:wait("#never-there", { timeout_ms = 400 })
	expect(app:count("#control-widget-map >> .orch-graph2-node")).to_be(before + 1)
end)

test("graph2 node style: provider glyphs where there's room, dots zoomed out", function()
	local app = open(1440)
	expect(app:state("control_map_graph2_stats").glyphs).to_be(0)
	app:command("map_graph2_provider")
	app:frames(5)
	local s = app:state("control_map_graph2_stats")
	expect(s.glyphs > 80).to_be(true)
	-- (Zoomed out past the glyphs' zoom: dots, drawn once more for it.)
	app:command("map_graph2_zoom_out")
	app:command("map_graph2_zoom_out")
	app:frames(5)
	expect(app:state("control_map_graph2_stats").glyphs).to_be(0)
	app:command("map_graph2_dots")
	app:frames(2)
end)

test("graph2: off screen, nothing lays out, draws or follows; back, it shows the world as it is", function()
	local app = open(1440)
	app:frames(10)
	-- (The window too short to show the Map: it's scrolled out of view.)
	app:resize(1440, 120)
	app:frames(30)
	local a = app:state("control_map_graph2_stats")
	local t0 = os.clock()
	for i = 1, 600 do
		if i % 150 == 0 then
			app:command(if i % 300 == 0 then "map_big_more" else "map_big_report")
		end
		app:frames(1)
	end
	local ms = (os.clock() - t0) * 1000
	local b = app:state("control_map_graph2_stats")
	print(`GRAPH2PERF off-screen 600 frames, 2 changes + 2 reports: total={math.floor(ms)}ms layouts={b.layouts - a.layouts} draws={b.draws - a.draws} overlay={b.overlay - a.overlay}`)
	expect(b.layouts).to_be(a.layouts)
	expect(b.draws).to_be(a.draws)
	expect(b.overlay).to_be(a.overlay)
	expect(app:count(".orch-graph2-node")).to_be(0)
	-- Back: laid out once for the world now, drawn, every thread there.
	app:resize(1440, 2400)
	app:wait("#never-there", { timeout_ms = 500 })
	local c = app:state("control_map_graph2_stats")
	expect(c.layouts).to_be(b.layouts + 1)
	expect(c.draws > b.draws).to_be(true)
	expect(app:count("#control-widget-map >> .orch-graph2-node")).to_be(107)
end)

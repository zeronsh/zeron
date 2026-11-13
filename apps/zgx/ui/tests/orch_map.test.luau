-- The Map's three views (ds/orch_map) on account/orchestration_sample's
-- busier world (ui/orch_map_fixture.luau): Topology's wiring exact at any
-- width, the Timeline's lanes and spans, the Machines blocks; a view chosen
-- from the widget's menu kept; a click on any thread opening it; a state
-- shown 300 ms at least.

local Layout = require("@ds/orch_map_layout")
local Timeline = require("@ds/orch_map_spans")
local model = require("@ds/orch_map_model")
local data = require("@ds/orch_data")
local sample = require("@account/orchestration_sample")

local WIDTHS = { 1600, 1200, 900, 700, 520 }

local function open(w: number?, h: number?): any
	local app = mount("orch_map_fixture.luau")
	app:resize(w or 1440, h or 2400)
	app:frames(30)
	return app
end

-- The first line of the tree holding `text`: its box.
local function box_of(app: any, within: string, text: string): (number, number, number, number)
	for line in app:tree(within):gmatch("[^\n]+") do
		if line:find(text, 1, true) then
			local x, y, w, h = line:match(" (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
			return tonumber(x) :: number, tonumber(y) :: number, tonumber(w) :: number, tonumber(h) :: number
		end
	end
	error(`nothing holds {text}`)
end

-- A node's box: x, y, w, h.
local function box(app: any, target: string): (number, number, number, number)
	local line = app:tree(target):match("[^\n]+")
	local x, y, w, h = line:match(" (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
	return tonumber(x) :: number, tonumber(y) :: number, tonumber(w) :: number, tonumber(h) :: number
end

-- The fleet as the page hands it to the Map.
local function world(): any
	local f = (sample :: any).fleet
	local sessions_now = { chats = {}, children = {} }
	for _, c in f.chats do
		table.insert(sessions_now.chats, c)
	end
	return { now = f.now, chats = data.chats(sessions_now), devices = f.devices, projects = f.projects, loads = f.loads, here = "dev-desk" }
end

local function title_of(id: string): string
	for _, c in (sample :: any).fleet.chats do
		if c.id == id then
			return c.title
		end
	end
	return ""
end

local function view(app: any, variant: string)
	app:command(`map_{variant}`)
	app:frames(20)
end

test("topology: each rail drops from its orchestrator's dot; each elbow ends where its row starts, at its middle, at any width", function()
	for _, w in WIDTHS do
		local l = Layout.layout(world(), w)
		local node: { [string]: any } = {}
		local crew: { [string]: { any } } = {}
		for _, r in l.rows do
			expect(r.x + r.w <= w).to_be(true)
			if r.kind == "orchestrator" then
				node[r.id] = r
			elseif r.kind == "crew" then
				local o = r.orchestrator :: string
				crew[o] = crew[o] or {}
				table.insert(crew[o], r)
			end
		end
		-- (The four orchestrators with crew, and the Brain dump.)
		expect(#l.rails).to_be(5)
		for _, rail in l.rails do
			local n = node[rail.orchestrator]
			-- Its dot: on the rail, the rail's start.
			expect(rail.x).to_be(n.x + Layout.DOT)
			expect(rail.dot).to_be(n.mid)
			-- One elbow a crew row, none into the orchestrator's own.
			expect(#rail.branches).to_be(#crew[rail.orchestrator])
			for _, b in rail.branches do
				local hit = false
				for _, r in crew[rail.orchestrator] do
					if r.mid == b.y and b.to == r.x - Layout.GAP then
						hit = true
					end
				end
				expect(hit).to_be(true)
				expect(b.y ~= n.mid).to_be(true)
			end
			-- Its lines start on the rail; the elbows end at their rows; its
			-- runs stay clear of its dot, and no row not its own sits on a
			-- stretch of it (it goes by a channel left of the rows there).
			local ends = 0
			for _, pts in Layout.lines(rail) do
				-- (Or in its channel, after going under a machine's head.)
				expect(pts[1] == rail.x or pts[1] < l.content_x).to_be(true)
				if pts[#pts - 1] > rail.x then
					ends += 1
				else
					for _, y in { pts[2], pts[#pts] } do
						expect(math.abs(y - rail.dot) >= Layout.DOT_R).to_be(true)
					end
					local min_x, jogs = math.huge, false
					for i = 1, #pts - 1, 2 do
						min_x = math.min(min_x, pts[i])
						jogs = jogs or pts[i] ~= rail.x
					end
					-- (Into its channel and back: left of every row.)
					expect(not jogs or min_x < l.content_x).to_be(true)
					for i = 1, #pts - 3, 2 do
						if pts[i] == rail.x and pts[i + 2] == rail.x then
							local lo, hi = math.min(pts[i + 1], pts[i + 3]), math.max(pts[i + 1], pts[i + 3])
							for _, r in l.rows do
								if r.orchestrator ~= rail.orchestrator then
									expect(r.mid > lo and r.mid < hi).to_be(false)
								end
							end
						end
					end
				end
			end
			expect(ends).to_be(#rail.branches)
			-- Every orchestrator starts where its peers do.
			expect(n.x).to_be(l.content_x)
		end
	end
end)

test("a machine's head is the same line in every view, never wrapped", function()
	local PARTS = { "orch-machine-head", "orch-machine-head-icon", "orch-machine-head-name", "orch-machine-head-platform", "orch-machine-head-meter", "orch-machine-head-figure" }
	local LEFT = 4
	for _, w in { 1440, 900 } do
		local app = open(w, 2400)
		local function boxes(within: string): { { number } }
			local out = {}
			for _, part in PARTS do
				local head = `{within} >> .orch-machine-head "desk"`
				local x, y, bw, bh = box(app, if part == "orch-machine-head" then head else `{head} >> .{part}`)
				table.insert(out, { x, y, bw, bh })
			end
			return out
		end
		view(app, "topology")
		local topo = boxes("#control-widget-map")
		-- This device: its dot on the mark's corner.
		expect(app:has('#control-widget-map >> .orch-machine-head "desk" >> .orch-machine-head-here')).to_be(true)
		expect(app:has('#control-widget-map >> .orch-machine-head "macbook" >> .orch-machine-head-here')).to_be(false)
		view(app, "timeline")
		local tl = boxes("#control-widget-map")
		view(app, "machines")
		local mc = boxes("#control-widget-map")
		local block = boxes("#control-widget-machines")
		-- One line: its height, its name's a single line.
		expect(topo[1][4]).to_be(28)
		expect(topo[3][4] <= 20).to_be(true)
		for i = 1, #PARTS do
			-- Topology and Timeline: the very same place and size, all of it.
			expect(tl[i][1]).to_be(topo[i][1])
			expect(tl[i][3]).to_be(topo[i][3])
			expect(tl[i][4]).to_be(topo[i][4])
			-- In a block (the Map's Machines, the Control room's): its left
			-- part the same from its own left edge; every part on one line.
			for _, other: { { number } } in { mc, block } :: { { { number } } } do
				if i <= LEFT then
					expect(other[i][1] - other[1][1]).to_be(topo[i][1] - topo[1][1])
				end
				expect(other[i][4]).to_be(topo[i][4])
				expect(other[i][2] - other[1][2]).to_be(topo[i][2] - topo[1][2])
			end
		end
		-- …and its load at the line's end, over the end of its CPU line.
		for _, other: { { number } } in { mc, block } :: { { { number } } } do
			local fig = other[#PARTS]
			expect(math.abs((fig[1] + fig[3]) - (other[1][1] + other[1][3])) < 0.5).to_be(true)
		end
		local lx, _ly, lw = box(app, "#control-widget-machines >> .orch-machine-line[1]")
		expect(math.abs((lx + lw) - (block[1][1] + block[1][3])) < 0.5).to_be(true)
	end
end)

test("topology: crew sit indented right of their orchestrator", function()
	for _, w in WIDTHS do
		local l = Layout.layout(world(), w)
		local node: { [string]: any } = {}
		for _, r in l.rows do
			if r.kind == "orchestrator" then
				node[r.id] = r
			end
		end
		for _, r in l.rows do
			if r.kind == "crew" then
				local n = node[r.orchestrator :: string]
				expect(r.x >= n.x + Layout.DOT + Layout.INDENT).to_be(true)
			end
		end
	end
	-- One orchestrator, its two crew on its machine: the live case.
	local f = (sample :: any).fleet
	local small = world()
	small.devices = { f.devices[1] }
	local now = table.clone(f.now)
	now.orchestrators = { f.now.orchestrators[5] }
	now.crew = {}
	for _, m in f.now.crew do
		if m.orchestrator_id == "orc-tally" and m.chat_id ~= "th-tally-docs" then
			table.insert(now.crew, m)
		end
	end
	small.now = now
	local chats = {}
	for id, c in small.chats do
		if id == "orc-tally" or id == "th-tally-lines" or id == "th-tally-ci" then
			c = table.clone(c)
			c.device = "dev-desk"
			c.project = "p-zeron-desk"
			chats[id] = c
		end
	end
	small.chats = chats
	local l = Layout.layout(small, 900)
	expect(#l.rows).to_be(3)
	expect(l.rows[1].kind).to_be("orchestrator")
	expect(#l.rails).to_be(1)
	local rail = l.rails[1]
	expect(rail.x).to_be(l.rows[1].x + Layout.DOT)
	expect(l.rows[2].x).to_be(rail.x + Layout.INDENT)
	expect(l.rows[3].x).to_be(rail.x + Layout.INDENT)
	expect(rail.top).to_be(l.rows[1].mid)
	expect(rail.bottom).to_be(l.rows[3].mid)
end)

test("topology: what the Brain dump started hangs under it, dashed, across machines", function()
	local l = Layout.layout(world(), 1200)
	local brain: any = nil
	for _, r in l.rails do
		if r.orchestrator == "orc-brain" then
			brain = r
		end
	end
	expect(brain ~= nil).to_be(true)
	expect(brain.dashed).to_be(true)
	-- Its three threads: on the macbook, this desk and the Cloud machine.
	expect(#brain.branches).to_be(3)
	local lane_of: { [string]: string } = {}
	for _, r in l.rows do
		lane_of[r.id] = r.lane
	end
	expect(lane_of["th-dev-names"]).to_be("dev-mac")
	expect(lane_of["th-wg-audit"]).to_be("dev-cloud")
	expect(lane_of["th-omarchy"]).to_be("dev-desk")
end)

test("topology: crew on another machine are wired across to it", function()
	local l = Layout.layout(world(), 1200)
	local lane_of: { [string]: any } = {}
	for _, lane in l.lanes do
		lane_of[lane.id] = lane
	end
	local function rail(o: string): any
		for _, r in l.rails do
			if r.orchestrator == o then
				return r
			end
		end
		return nil
	end
	-- Orchestrators lives on desk; two of its crew run on the macbook.
	local orch = rail("orc-orch")
	local desk, mac = lane_of["dev-desk"], lane_of["dev-mac"]
	expect(orch.top >= desk.y and orch.top < desk.y + desk.h).to_be(true)
	expect(orch.bottom >= mac.y and orch.bottom < mac.y + mac.h).to_be(true)
	-- Exnomic VPN's rail reaches its build on the Cloud machine.
	local vpn = rail("orc-vpn")
	expect(vpn.bottom >= lane_of["dev-cloud"].y).to_be(true)
end)

test("topology: rows are drawn where their branches end, at several widths", function()
	for _, w in { 1440, 1100, 900, 700 } do
		local app = open(w, 2400)
		local fx, fy, fw = box(app, "#orch-map-field")
		local l = Layout.layout(world(), fw)
		-- An orchestrator's row: its dot on its rail.
		for _, rail in l.rails do
			if rail.orchestrator == "orc-orch" or rail.orchestrator == "orc-reins" then
				local name = if rail.orchestrator == "orc-orch" then "Orchestrators" else "Reins release"
				local x, y, _bw, bh = box_of(app, "#orch-map-field", `button "{name}"`)
				expect(math.abs((x - fx) + Layout.DOT - rail.x) < 0.5).to_be(true)
				expect(math.abs((y - fy) + bh / 2 - rail.dot) < 0.5).to_be(true)
			end
		end
		for _, id in { "th-ui-control", "th-engine-state", "th-tally-ci", "th-reins-win" } do
			for _, r in l.rows do
				if r.id == id then
					local x, y, _bw, bh = box_of(app, "#orch-map-field", `button "{title_of(id)}`)
					expect(math.abs((x - fx) - r.x) < 0.5).to_be(true)
					expect(math.abs((y - fy) + bh / 2 - r.mid) < 0.5).to_be(true)
				end
			end
		end
	end
end)

test("topology: each machine a lane, every thread at work a row", function()
	local app = open()
	expect(app:count("#control-widget-map >> .orch-lane")).to_be(4)
	-- Five orchestrators; twelve crew and three threads the Brain dump
	-- started, under it; one of the person's own at work.
	expect(app:count("#control-widget-map >> .orch-node")).to_be(5)
	expect(app:count("#control-widget-map >> .orch-pill")).to_be(15)
	expect(app:count("#control-widget-map >> .orch-thread")).to_be(1)
end)

test("topology: a crew row on another machine opens its thread", function()
	local app = open()
	app:click('#control-widget-map >> .orch-pill "Control room UI, Testing"')
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/th-ui-control")
end)

test("the view is chosen from the widget's menu and kept", function()
	local app = open()
	app:click('#control-widget-map >> role:button "Widget menu"')
	app:frames(10)
	app:click('role:menuitemcheckbox "Timeline"')
	app:frames(20)
	expect(app:state("control_map").variant).to_be("timeline")
	expect(app:has("#control-widget-map >> .orch-map-timeline")).to_be(true)
	expect(app:has("#control-widget-map >> .orch-map-topology")).to_be(false)
	app:click('#control-widget-map >> role:button "Widget menu"')
	app:frames(10)
	app:click('role:menuitemcheckbox "Machines"')
	app:frames(20)
	expect(app:state("control_map").variant).to_be("machines")
	expect(app:count("#control-widget-map >> .orch-mc-block")).to_be(4)
	-- The widget's own entries still follow.
	app:click('#control-widget-map >> role:button "Widget menu"')
	app:frames(10)
	expect(app:has('"Move up"')).to_be(true)
end)

test("timeline: a lane a thread, under its machine and project", function()
	local app = open()
	view(app, "timeline")
	expect(app:count("#control-widget-map >> .orch-tl-machine")).to_be(4)
	expect(app:count("#control-widget-map >> .orch-tl-lane")).to_be(21)
	expect(app:count("#control-widget-map >> .orch-tl-project")).to_be(7)
	-- What it does now, at its end.
	expect(app:has('#control-widget-map >> "Awaiting Input"')).to_be(true)
	expect(app:has('#control-widget-map >> "zgx test ui control_"')).to_be(true)
end)

test("timeline: the hour or the last 15 minutes, kept", function()
	local app = open()
	view(app, "timeline")
	expect(app:state("control_map").window).to_be(15)
	app:click('#control-widget-map >> .orch-tl-window "Last hour"')
	app:frames(10)
	expect(app:state("control_map").window).to_be(60)
	expect(app:state("control_map").variant).to_be("timeline")
end)

test("timeline: a lane opens its thread; an orchestrator's, its page", function()
	local app = open()
	view(app, "timeline")
	app:click('#control-widget-map >> .orch-tl-lane "Windows installer, Building"')
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/th-reins-win")
	app:click('#control-widget-map >> .orch-tl-lane "Tally"')
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/orchestrator/orc-tally")
end)

test("timeline: spans from what the host kept, ticks where things happened", function()
	local now = 10000
	local entries = {
		{ at = now - 1000, status = "working", kind = "testing" },
		{ at = now - 800, status = "failed" },
		{ at = now - 700, status = "working", kind = "merging" },
		{ at = now - 400, status = "working", kind = "editing" },
	}
	local chat = { status = "working", activity = { kind = "editing", since = now - 400 } }
	local spans, ticks = Timeline.spans_of(entries, chat, now - 900, now)
	-- The first span starts at the window's start.
	expect(spans[1].t0).to_be(now - 900)
	expect(spans[1].kind).to_be("testing")
	expect(spans[#spans].t1).to_be(now)
	expect(spans[#spans].kind).to_be("editing")
	local kinds = {}
	for _, t in ticks do
		kinds[t.kind] = t.t
	end
	expect(kinds.failed).to_be(now - 800)
	expect(kinds.merged).to_be(now - 400)
	-- Its state now, newer than what was kept: added.
	local s2 = Timeline.spans_of(entries, { status = "input" }, now - 900, now)
	expect(s2[#s2].status).to_be("input")
	-- No history at all: what it does now, since it began.
	local s3 = Timeline.spans_of(nil, chat, now - 900, now)
	expect(#s3).to_be(1)
	expect(s3[1].t0).to_be(now - 400)
end)

test("machines: a block a machine, in as many columns as fit", function()
	local function columns(app: any): number
		local xs: { [string]: boolean } = {}
		local n = 0
		for line in app:tree("#control-widget-map"):gmatch("[^\n]+") do
			if line:find(".orch-mc-block", 1, true) or line:find("orch-mc-block", 1, true) then
				local x = line:match(" (%-?[%d%.]+),") or ""
				if not xs[x] then
					xs[x] = true
					n += 1
				end
			end
		end
		return n
	end
	local app = open(1600, 2400)
	view(app, "machines")
	expect(app:count("#control-widget-map >> .orch-mc-block")).to_be(4)
	-- (Tall enough that the Map is in view under Needs you and Progress.)
	app:resize(700, 3400)
	app:frames(20)
	expect(app:count("#control-widget-map >> .orch-mc-block")).to_be(4)
	local _ = columns
	-- Crew rows open their thread; an orchestrator's row, its page.
	app:click('#control-widget-map >> .orch-pill "Exporter image, Building"')
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/th-vpn-exporter")
	app:click('#control-widget-map >> .orch-node "Reins release"')
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/orchestrator/orc-reins")
end)

test("every view draws and follows a resize", function()
	local app = open(1440, 2400)
	for _, v in { "topology", "timeline", "machines" } do
		view(app, v)
		for _, w in WIDTHS do
			app:resize(w, 2400)
			app:frames(10)
			expect(app:has(`#control-widget-map >> .orch-map-{v}`)).to_be(true)
			local _x, _y, mw = box(app, "#control-widget-map")
			expect(mw <= w).to_be(true)
		end
	end
end)

test("a thread's activity shows 300 ms at least, then the latest", function()
	local app = open()
	local building = '#control-widget-map >> .orch-pill "Engine: orchestration state, Building"'
	local merging = '#control-widget-map >> .orch-pill "Engine: orchestration state, Merging"'
	-- (Testing has shown long enough; then a frame is a 60th of a second.)
	app:wait("#never-there", { timeout_ms = 400 })
	app:command("fixture.flicker")
	app:frames(2)
	expect(app:has(building)).to_be(true)
	-- Merging came 60 ms after: held behind Building for its 300 ms…
	app:frames(12)
	expect(app:has(building)).to_be(true)
	expect(app:has(merging)).to_be(false)
	-- …then shown.
	app:frames(12)
	expect(app:has(merging)).to_be(true)
end)

test("the model puts each thread on its machine, by project", function()
	local f = model.fleet(world())
	local names = {}
	for _, m in f.machines do
		table.insert(names, m.name)
	end
	-- This one first, the Cloud machine last.
	expect(names[1]).to_be("desk")
	expect(names[#names]).to_be("Cloud")
	expect(f.threads["th-reins-win"].device).to_be("dev-forge")
	expect(f.threads["th-reins-win"].project).to_be("reins")
	expect(f.threads["th-zgx-mem"].kind).to_be("thread")
	-- Done and idle threads of the person's own aren't shown.
	expect(f.threads["th-titlebar"]).to_be(nil)
end)

test("a remote thread's turn ending shows in every view at once, no activity ever written", function()
	local thread = "Verify unique device names"
	local function state_in(app: any, within: string): string
		for line in app:tree(within):gmatch("[^\n]+") do
			local label = line:match('^%s*@%d+ "([^"]*)"')
			if label then
				return label
			end
		end
		return ""
	end
	local app = open()
	app:command("fixture.remote_working")
	app:wait("#never-there", { timeout_ms = 400 })
	expect(state_in(app, `#control-widget-map >> .orch-pill "{thread}" >> .orch-state`)).to_be("Working")
	expect(app:has('#control-widget-map >> .orch-node "Brain dump" >> .orch-state')).to_be(true)
	expect(app:has(`#control-widget-machines >> .orch-machine-job "{thread}"`)).to_be(true)
	-- Its turn ends (its rows flickering as they come): idle for good.
	app:command("fixture.remote_flicker")
	app:wait("#never-there", { timeout_ms = 700 })
	for _, v in { "topology", "timeline", "machines" } do
		view(app, v)
		local row = if v == "timeline" then `.orch-tl-lane "{thread}"` else `.orch-pill "{thread}"`
		expect(app:has(`#control-widget-map >> {row}`)).to_be(true)
		expect(state_in(app, `#control-widget-map >> {row} >> .orch-state`)).to_be("")
	end
	view(app, "topology")
	expect(app:has('#control-widget-map >> .orch-node "Brain dump" >> .orch-state')).to_be(false)
	-- The Control room's Machines: no longer among what runs.
	expect(app:has(`#control-widget-machines >> .orch-machine-job "{thread}"`)).to_be(false)
end)

test("a chat of no project shows under its machine in every view; comms name every party", function()
	local app = open()
	app:command("fixture.projectless")
	app:frames(20)
	for _, v in { "topology", "timeline", "machines" } do
		view(app, v)
		local row = if v == "timeline" then '.orch-tl-lane "Hello World Agent"' else '.orch-thread "Hello World Agent"'
		expect(app:has(`#control-widget-map >> {row}`)).to_be(true)
	end
	-- Under its machine: in the Topology, in winbox's lane.
	view(app, "topology")
	local _, lane_y = box(app, '#control-widget-map >> .orch-machine-head "winbox"')
	local _, row_y = box(app, '#control-widget-map >> .orch-thread "Hello World Agent"')
	local _, next_y = box(app, '#control-widget-map >> .orch-machine-head "Cloud"')
	expect(row_y > lane_y and row_y < next_y).to_be(true)
	-- Comms: each party by name, a thread not yet made or unknown by what it is.
	expect(app:has('#control-widget-comms >> "Hello World Agent"')).to_be(true)
	expect(app:has('#control-widget-comms >> "New thread"')).to_be(true)
	for line in app:tree("#control-widget-comms"):gmatch("[^\n]+") do
		expect(line:find('text ""', 1, true) == nil).to_be(true)
	end
end)

local Sim = require("@ds/orch_map_sim")

test("graph: the web settles sound, its lines join the right nodes", function()
	local g = Sim.build(world())
	Sim.settle(g, 800)
	for _, n in g.nodes do
		expect(n.x == n.x and n.y == n.y).to_be(true)
		expect(math.abs(n.x) < 3000 and math.abs(n.y) < 3000).to_be(true)
	end
	local has: { [string]: boolean } = {}
	for _, e in g.edges do
		expect(g.index[e.a] ~= nil and g.index[e.b] ~= nil).to_be(true)
		has[`{e.kind}:{e.a}>{e.b}`] = true
	end
	-- What runs where, what leads what, what the Brain dump started and
	-- handed parts to.
	expect(has["host:dev:dev-mac>th-ui-control"]).to_be(true)
	expect(has["control:orc-orch>th-ui-control"]).to_be(true)
	expect(has["started:orc-brain>th-dev-names"]).to_be(true)
	expect(has["started:orc-brain>th-wg-audit"]).to_be(true)
	expect(has["started:orc-brain>th-omarchy"]).to_be(true)
	expect(has["routed:orc-brain>orc-reins"]).to_be(true)
	expect(has["routed:orc-brain>th-ui-orch"]).to_be(true)
	-- Still once settled: a step moves nothing.
	expect(Sim.step(g)).to_be(false)
end)

test("graph: every node shown in the field, lit with its neighbors under the pointer, a click opens it", function()
	local app = open(1440, 2400)
	view(app, "graph")
	app:frames(90)
	local g = Sim.build(world())
	expect(app:count("#control-widget-map >> .orch-graph-node")).to_be(#g.nodes)
	local fx, fy, fw, fh = box(app, "#orch-graph-field")
	for line in app:tree("#control-widget-map"):gmatch("[^\n]+") do
		local x, y, w, h = line:match(" (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
		if line:find("Control room UI", 1, true) and x then
			local cx = (tonumber(x) :: number) + (tonumber(w) :: number) / 2
			local cy = (tonumber(y) :: number) + (tonumber(h) :: number) / 2
			expect(cx > fx and cx < fx + fw and cy > fy and cy < fy + fh).to_be(true)
		end
	end
	app:hover('#control-widget-map >> .orch-graph-thread ~"Control room UI"')
	app:frames(5)
	expect(app:state("control_map_graph_hover")).to_be("th-ui-control")
	expect(app:has("#control-widget-map >> .orch-graph-card")).to_be(true)
	local near = Sim.neighbors(g, "th-ui-control")
	expect(near["orc-orch"]).to_be(true)
	expect(near["dev:dev-mac"]).to_be(true)
	expect(near["th-engine-state"]).to_be(nil)
	app:click('#control-widget-map >> .orch-graph-thread ~"Control room UI"')
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/th-ui-control")
end)

test("graph: a node dropped stays where it was dropped", function()
	local app = open(1440, 2400)
	view(app, "graph")
	app:frames(90)
	app:drag('#control-widget-map >> .orch-graph-thread ~"Exporter image"', '#control-widget-map >> .orch-graph-device "winbox"')
	app:frames(30)
	local pins = app:state("control_map_graph_pins")
	expect(pins["th-vpn-exporter"] ~= nil).to_be(true)
	-- Reset: back to the web.
	app:click("#orch-graph-reset")
	app:frames(5)
	expect(app:state("control_map_graph_pins")["th-vpn-exporter"]).to_be(nil)
end)

test("started_by from the host: under who started it, in the Topology and the Graph", function()
	local w = world()
	local chats = table.clone(w.chats)
	-- A thread an orchestrator spawned, not (yet) in its crew; another the
	-- Brain dump started, its dump unknown here.
	chats["th-spawned"] = { id = "th-spawned", title = "Spawned by Tally", device = "dev-cloud", status = "working", started_by = { chat_id = "orc-tally" } }
	chats["th-bd"] = { id = "th-bd", title = "Started from a dump", device = "dev-mac", status = "working", started_by = { chat_id = "orc-brain", dump_id = "dp-x" } }
	w.chats = chats
	local l = Layout.layout(w, 1200)
	local of: { [string]: string } = {}
	for _, rail in l.rails do
		for _, b in rail.branches do
			for _, r in l.rows do
				if r.mid == b.y and r.x == b.to + Layout.GAP then
					of[r.id] = rail.orchestrator
				end
			end
		end
	end
	expect(of["th-spawned"]).to_be("orc-tally")
	expect(of["th-bd"]).to_be("orc-brain")
	local g = Sim.build(w)
	local has: { [string]: boolean } = {}
	for _, e in g.edges do
		has[`{e.kind}:{e.a}>{e.b}`] = true
	end
	expect(has["started:orc-tally>th-spawned"]).to_be(true)
	expect(has["started:orc-brain>th-bd"]).to_be(true)
end)

-- The Map's three views (ds/orch_map) on account/orchestration_sample's
-- busier world (ui/orch_map_fixture.luau): Topology's wiring exact at any
-- width, the Timeline's lanes and spans, the Machines blocks; a view chosen
-- from the widget's menu kept; a click on any thread opening it; a state
-- shown 300 ms at least.

local Layout = require("@orchestrators/ds/orch_map_layout")
local Timeline = require("@orchestrators/ds/orch_map_spans")
local model = require("@orchestrators/ds/orch_map_model")
local data = require("@orchestrators/ds/orch_data")
local sample = require("@orchestrators/account/orchestration_sample")

local WIDTHS = { 1600, 1200, 900, 700, 520 }

local function open(w: number?, h: number?): any
	local app = mount("orch_map_fixture.luau")
	app:command("map_only")
	app:resize(w or 1440, h or 2400)
	-- (These read the Topology's rows and pills: the Map opens on Graph
	-- (force) now, and its tab on a view it lists.)
	app:set_state("control_map", { variant = "topology", window = 15 })
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
	-- (Judged by the Map's "Show chats" as the fixture's is: the default, at
	-- the sample's own time, its chats as old as the fixture's.)
	return { now = f.now, chats = data.chats(sessions_now), devices = f.devices, projects = f.projects, loads = f.loads, here = "dev-desk",
		show = "recent", clock = f.NOW }
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
			-- Its dot: on the rail, the rail's start; or, a rail across
			-- machines, in a column of its own left of every row, a hop from
			-- its dot into it.
			local column = rail.x ~= n.x + Layout.DOT
			if column then
				expect(rail.x < l.content_x).to_be(true)
			end
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
				expect(pts[1] == rail.x or pts[1] < l.content_x or (column and pts[1] == n.x + Layout.DOT)).to_be(true)
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
					for i = 1, if column then 0 else #pts - 3, 2 do
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
		local topo = boxes("#control-map")
		-- This device: its dot on the mark's corner.
		expect(app:has('#control-map >> .orch-machine-head "desk" >> .orch-machine-head-here')).to_be(true)
		expect(app:has('#control-map >> .orch-machine-head "macbook" >> .orch-machine-head-here')).to_be(false)
		view(app, "timeline")
		local tl = boxes("#control-map")
		view(app, "machines")
		local mc = boxes("#control-map")
		-- One line: its height, its name's a single line.
		expect(topo[1][4]).to_be(28)
		expect(topo[3][4] <= 20).to_be(true)
		for i = 1, #PARTS do
			-- Topology and Timeline: the very same place and size, all of it.
			expect(tl[i][1]).to_be(topo[i][1])
			expect(tl[i][3]).to_be(topo[i][3])
			expect(tl[i][4]).to_be(topo[i][4])
			-- In the Map's Machines: its left part the same from its own left
			-- edge; every part on one line.
			for _, other: { { number } } in { mc } :: { { { number } } } do
				if i <= LEFT then
					expect(other[i][1] - other[1][1]).to_be(topo[i][1] - topo[1][1])
				end
				expect(other[i][4]).to_be(topo[i][4])
				expect(other[i][2] - other[1][2]).to_be(topo[i][2] - topo[1][2])
			end
		end
		-- …and its load at the line's end, over the end of its CPU line.
		for _, other: { { number } } in { mc } :: { { { number } } } do
			local fig = other[#PARTS]
			expect(math.abs((fig[1] + fig[3]) - (other[1][1] + other[1][3])) < 0.5).to_be(true)
		end
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
				expect(math.abs((x - fx) - l.content_x) < 0.5).to_be(true)
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
	expect(app:count("#control-map >> .orch-lane")).to_be(4)
	-- Five orchestrators; twelve crew and three threads the Brain dump
	-- started, under it; one of the person's own at work.
	expect(app:count("#control-map >> .orch-node")).to_be(5)
	expect(app:count("#control-map >> .orch-pill")).to_be(15)
	expect(app:count("#control-map >> .orch-thread")).to_be(1)
end)

test("topology: a crew row on another machine opens its thread", function()
	local app = open()
	app:click('#control-map >> .orch-pill "Control room UI, Testing"')
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/th-ui-control")
end)

test("the view is chosen from the Map's menu and kept: the Graph or the Machines", function()
	local app = open()
	local function menu()
		app:click("#control-map >> .control-panel-menu >> role:button")
		app:frames(10)
	end
	menu()
	-- (Two views offered, no more.)
	expect(app:has('role:menuitemcheckbox "Timeline"')).to_be(false)
	expect(app:has('role:menuitemcheckbox "Topology"')).to_be(false)
	app:click('role:menuitemcheckbox "Machines"')
	app:frames(20)
	expect(app:state("control_map").variant).to_be("machines")
	expect(app:count("#control-map >> .orch-mc-block")).to_be(4)
	menu()
	app:click('role:menuitemcheckbox "Graph"')
	app:frames(20)
	expect(app:state("control_map").variant).to_be("force")
	-- (No widget's own entries: the Map is a tab.)
	menu()
	expect(app:has('"Move up"')).to_be(false)
end)

test("timeline: a lane a thread, under its machine and project", function()
	local app = open()
	view(app, "timeline")
	expect(app:count("#control-map >> .orch-tl-machine")).to_be(4)
	expect(app:count("#control-map >> .orch-tl-lane")).to_be(21)
	expect(app:count("#control-map >> .orch-tl-project")).to_be(7)
	-- What it does now, at its end.
	expect(app:has('#control-map >> "Awaiting Input"')).to_be(true)
	expect(app:has('#control-map >> "zgx test ui control_"')).to_be(true)
end)

test("timeline: the hour or the last 15 minutes, kept", function()
	local app = open()
	view(app, "timeline")
	expect(app:state("control_map").window).to_be(15)
	app:click('#control-map >> .orch-tl-window "Last hour"')
	app:frames(10)
	expect(app:state("control_map").window).to_be(60)
	expect(app:state("control_map").variant).to_be("timeline")
end)

test("timeline: a lane opens its thread; an orchestrator's, its page", function()
	local app = open()
	view(app, "timeline")
	app:click('#control-map >> .orch-tl-lane "Windows installer, Building"')
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/th-reins-win")
	app:click('#control-map >> .orch-tl-lane "Tally"')
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
	-- (A merge step finished: a Done tick, not a mark of its own.)
	expect(kinds.done).to_be(now - 400)
	expect(kinds.merged).to_be(nil)
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
		for line in app:tree("#control-map"):gmatch("[^\n]+") do
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
	expect(app:count("#control-map >> .orch-mc-block")).to_be(4)
	-- (Tall enough that the Map is in view under Needs you and Progress.)
	app:resize(700, 3400)
	app:frames(20)
	expect(app:count("#control-map >> .orch-mc-block")).to_be(4)
	local _ = columns
	-- (One column at this width: the Map, at its bound, scrolls to them.)
	local function into_view(target: string)
		local _, sy, _, sh = box(app, "#control-map >> .orch-map-scroll")
		local _, y = box(app, target)
		if y < sy or y > sy + sh - 30 then
			app:scroll("#control-map >> .orch-map-scroll", y - sy - sh / 2)
			app:frames(10)
		end
	end
	-- Crew rows open their thread; an orchestrator's row, its page.
	into_view('#control-map >> .orch-pill "Exporter image, Building"')
	app:click('#control-map >> .orch-pill "Exporter image, Building"')
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/th-vpn-exporter")
	into_view('#control-map >> .orch-node "Reins release"')
	app:click('#control-map >> .orch-node "Reins release"')
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
			expect(app:has(`#control-map >> .orch-map-{v}`)).to_be(true)
			local _x, _y, mw = box(app, "#control-map")
			expect(mw <= w).to_be(true)
		end
	end
end)

test("a thread's activity shows 300 ms at least, then the latest", function()
	local app = open()
	local building = '#control-map >> .orch-pill "Engine: orchestration state, Building"'
	local merging = '#control-map >> .orch-pill "Engine: orchestration state, Merging"'
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

test("nothing archived is on the Map: an orchestrator, a crew thread, nor a thread of the person's own", function()
	local f = (sample :: any).fleet
	local gone = { ["orc-vpn"] = true, ["th-engine-flow"] = true, ["th-zgx-mem"] = true }
	-- (Archived as the host lists them: under `archived`, unflagged.)
	local sessions_now = { chats = {}, children = {}, archived = {} }
	for _, c in f.chats do
		table.insert(if gone[c.id] then sessions_now.archived else sessions_now.chats, c)
	end
	local w = { now = f.now, chats = data.chats(sessions_now), devices = f.devices, projects = f.projects, loads = f.loads, here = "dev-desk" }
	local fleet = model.fleet(w)
	for id in gone do
		expect(fleet.threads[id]).to_be(nil)
	end
	expect(fleet.orchestrators["orc-vpn"]).to_be(nil)
	expect(fleet.threads["th-engine-state"] ~= nil).to_be(true)
	expect(fleet.orchestrators["orc-orch"] ~= nil).to_be(true)
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
	expect(state_in(app, `#control-map >> .orch-pill "{thread}" >> .orch-state`)).to_be("Working")
	expect(app:has('#control-map >> .orch-node "Brain dump" >> .orch-state')).to_be(true)
	-- Its turn ends (its rows flickering as they come): idle for good.
	app:command("fixture.remote_flicker")
	app:wait("#never-there", { timeout_ms = 700 })
	for _, v in { "topology", "timeline", "machines" } do
		view(app, v)
		local row = if v == "timeline" then `.orch-tl-lane "{thread}"` else `.orch-pill "{thread}"`
		expect(app:has(`#control-map >> {row}`)).to_be(true)
		expect(state_in(app, `#control-map >> {row} >> .orch-state`)).to_be("")
	end
	view(app, "topology")
	expect(app:has('#control-map >> .orch-node "Brain dump" >> .orch-state')).to_be(false)
end)

test("a chat of no project shows under its machine in every view", function()
	local app = open()
	app:command("fixture.projectless")
	app:frames(20)
	for _, v in { "topology", "timeline", "machines" } do
		view(app, v)
		local row = if v == "timeline" then '.orch-tl-lane "Hello World Agent"' else '.orch-thread "Hello World Agent"'
		expect(app:has(`#control-map >> {row}`)).to_be(true)
	end
	-- Under its machine: in the Topology, in winbox's lane.
	view(app, "topology")
	local _, lane_y = box(app, '#control-map >> .orch-machine-head "winbox"')
	local _, row_y = box(app, '#control-map >> .orch-thread "Hello World Agent"')
	local _, next_y = box(app, '#control-map >> .orch-machine-head "Cloud"')
	expect(row_y > lane_y and row_y < next_y).to_be(true)
end)

local Sim = require("@orchestrators/ds/orch_map_sim")

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
	expect(app:count("#control-map >> .orch-graph-node")).to_be(#g.nodes)
	local fx, fy, fw, fh = box(app, "#orch-graph-field")
	for line in app:tree("#control-map"):gmatch("[^\n]+") do
		local x, y, w, h = line:match(" (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
		if line:find("Control room UI", 1, true) and x then
			local cx = (tonumber(x) :: number) + (tonumber(w) :: number) / 2
			local cy = (tonumber(y) :: number) + (tonumber(h) :: number) / 2
			expect(cx > fx and cx < fx + fw and cy > fy and cy < fy + fh).to_be(true)
		end
	end
	app:hover('#control-map >> .orch-graph-thread ~"Control room UI"')
	app:frames(5)
	expect(app:state("control_map_graph_hover")).to_be("th-ui-control")
	expect(app:has("#control-map >> .orch-graph-card")).to_be(true)
	local near = Sim.neighbors(g, "th-ui-control")
	expect(near["orc-orch"]).to_be(true)
	expect(near["dev:dev-mac"]).to_be(true)
	expect(near["th-engine-state"]).to_be(nil)
	app:click('#control-map >> .orch-graph-thread ~"Control room UI"')
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/th-ui-control")
end)

test("graph: a node dropped stays where it was dropped", function()
	local app = open(1440, 2400)
	view(app, "graph")
	app:frames(90)
	app:drag('#control-map >> .orch-graph-thread ~"Exporter image"', '#control-map >> .orch-graph-device "winbox"')
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

-- A secondary click's menu for `id` in the view showing.
local function menu_for(app: any, id: string)
	app:set_state("map_menu_id", id)
	app:command("map_menu")
	app:frames(10)
end

test("a secondary click on a thread, an orchestrator or a machine opens its menu, in every view", function()
	for _, v in { "topology", "timeline", "machines", "graph" } do
		local app = open(1440, 2400)
		view(app, v)
		app:frames(if v == "graph" then 80 else 10)
		-- A machine.
		menu_for(app, "dev:dev-mac")
		expect(app:has('"Rename device"')).to_be(true)
		-- An orchestrator: its own Pause, Settings and Archive.
		menu_for(app, "orc-orch")
		for _, item in { "Open", "Pause", "Settings…", "Archive" } do
			expect(app:has(`"{item}"`)).to_be(true)
		end
		expect(app:has('"Release from crew"')).to_be(false)
		-- A crew thread at work.
		menu_for(app, "th-ui-control")
		for _, item in { "Open", "Stop", "What's up?", "Pin to Progress", "Rename", "Move to", "Copy", "Sleep", "Release from crew", "Archive" } do
			expect(app:has(`"{item}"`)).to_be(true)
		end
		expect(app:has('"Delete"')).to_be(false)
		expect(app:has('"Make orchestrator"')).to_be(false)
	end
end)

test("the menu's Stop and Archive do what they say", function()
	local app = open(1440, 2400)
	local function choose(id: string, label: string)
		app:set_state("map_menu_id", id)
		app:set_state("map_menu_pick", label)
		app:command("map_menu_choose")
		app:frames(5)
	end
	choose("th-ui-control", "Stop")
	expect(string.find(app:state("calls"), "stop:th-ui-control", 1, true) ~= nil).to_be(true)
	-- (A thread done: Archive is there to take.)
	choose("th-tally-docs", "Archive")
	expect(string.find(app:state("calls"), "archive:th-tally-docs", 1, true) ~= nil).to_be(true)
	choose("th-ui-control", "Release from crew")
	expect(string.find(app:state("calls"), "op:releaseFromCrew:th-ui-control", 1, true) ~= nil).to_be(true)
end)

test("topology: a rail running past a machine with nothing at work never crosses its words", function()
	local t = os.time()
	local sessions_now = {
		chats = {
			{ id = "orc-brain", title = "Brain dump", device = "hp1", updated = t - 30, created = t - 9000 },
			{ id = "th-codex", title = "Codex Slash Commands Missing", device = "hp", status = "done", updated = t - 60, created = t - 900, started_by = "orc-brain" },
		},
		children = {},
	}
	local orch = { id = "orc-brain", kind = "brain_dump", name = "Brain dump", hue = 172, archived = false, paused = false }
	local w = {
		now = { orchestrators = { orch }, crew = {}, items = {}, dumps = {} },
		chats = data.chats(sessions_now),
		devices = {
			{ id = "hp1", name = "honeypot-1", platform = "linux", here = true, online = true },
			{ id = "win", name = "DESKTOP-3NFV6JC", platform = "windows", online = true },
			{ id = "m2a", name = "M2A", platform = "macos", online = true },
			{ id = "hp", name = "honeypot", platform = "linux", online = true },
		},
		projects = {},
		loads = {},
		here = "hp1",
	}
	local l = Layout.layout(w, 700)
	expect(#l.rails).to_be(1)
	local rail = l.rails[1]
	local empty = 0
	for _, lane in l.lanes do
		if lane.count == 0 then
			empty += 1
			-- (The rail passes it: its words start right of the rail, clear.)
			expect(lane.idle_x > rail.x + 6).to_be(true)
		end
	end
	expect(empty).to_be(2)
end)

test("topology: rails across machines keep a column each; where an elbow passes another's, that one breaks", function()
	local l = Layout.layout(world(), 1200)
	local cols = {}
	for _, rail in l.rails do
		if rail.x < l.content_x then
			table.insert(cols, rail)
		end
	end
	expect(#cols >= 2).to_be(true)
	for _, a in cols do
		for _, b in a.branches do
			for _, other in cols do
				if other ~= a and other.x > a.x and other.x < b.to then
					-- (No upright of `other` runs through the elbow's height.)
					for _, run in other.runs do
						for i = 1, #run - 3, 2 do
							if run[i] == other.x and run[i + 2] == other.x then
								local lo, hi = math.min(run[i + 1], run[i + 3]), math.max(run[i + 1], run[i + 3])
								expect(b.y > lo and b.y < hi).to_be(false)
							end
						end
					end
				end
			end
		end
	end
end)

test("a crew row is never blank: the chat's name, else its card's, else its role", function()
	local t = os.time()
	local sessions_now = {
		chats = { { id = "orc-z", title = "Zeron fixes", device = "hp1", updated = t, created = t - 900 } },
		children = { ["orc-z"] = {
			{ id = "th-a", parent = "orc-z", title = "", device = "hp1", child_kind = "crew", updated = t, created = t - 600 },
			{ id = "th-b", parent = "orc-z", title = "", device = "hp1", child_kind = "crew", updated = t, created = t - 600 },
		} },
	}
	local w = {
		now = {
			orchestrators = { { id = "orc-z", kind = "standard", name = "Zeron fixes", hue = 20, archived = false, paused = false } },
			crew = {
				{ chat_id = "th-a", orchestrator_id = "orc-z", item_id = "it-a", role = "", released = false },
				{ chat_id = "th-b", orchestrator_id = "orc-z", role = "scroll fix", released = false },
			},
			items = { { id = "it-a", title = "Board card declutter", order = 1 } },
			dumps = {},
		},
		chats = data.chats(sessions_now),
		devices = { { id = "hp1", name = "honeypot-1", platform = "linux", here = true, online = true } },
		projects = {},
		loads = {},
		here = "hp1",
	}
	local f = model.fleet(w)
	expect(f.threads["th-a"].title).to_be("Board card declutter")
	expect(f.threads["th-b"].title).to_be("scroll fix")
end)

test("timeline: the legend's events are always Report, Ask, Failed, Done", function()
	local app = open(1440, 1400)
	view(app, "timeline")
	app:frames(10)
	local tree = app:tree("#control-map >> .orch-tl-top")
	local at = {}
	for _, word in { "Report", "Ask", "Failed", "Done" } do
		local i = string.find(tree, `"{word}"`, 1, true)
		expect(i ~= nil).to_be(true)
		table.insert(at, i)
	end
	for k = 2, #at do
		expect(at[k] > at[k - 1]).to_be(true)
	end
	expect(string.find(tree, '"Merged"', 1, true)).to_be(nil)
end)

test("graph: a pulse travels its line as drawn, from mark edge to mark edge, every frame on it", function()
	local app = open(1440, 1400)
	view(app, "graph")
	app:set_state("control_map_graph_trace", true)
	app:frames(120)
	app:command("fixture.report")
	local seen, bowed, last_a = 0, 0, -1
	local first, final = nil, nil
	-- (Frame by frame till it's in: its clock is the app's, not the
	-- frames', so however quick a frame is drawn.)
	local start = os.clock()
	while os.clock() - start < 10 do
		app:frames(1)
		local p = (app:state("control_map_graph_pulse") or {})[1]
		if not p and seen > 0 then
			break
		end
		if p then
			seen += 1
			first = first or p
			final = p
			-- (On its line, to within the drawing's sampling.)
			expect(p.off < 2).to_be(true)
			bowed = math.max(bowed, p.chord)
			-- (Onward, never back.)
			expect(p.from_a >= last_a - 0.5).to_be(true)
			last_a = p.from_a
		end
	end
	expect(seen > 30).to_be(true)
	-- (The line bows; so does its pulse, off the straight chord.)
	expect(bowed > 4).to_be(true)
	-- (It starts where the line leaves the one mark and ends where it meets
	-- the other, not at their centres.)
	expect(first.from_a >= first.reach_a and first.from_a <= first.reach_a + 8).to_be(true)
	expect(final.from_b >= final.reach_b and final.from_b <= final.reach_b + 10).to_be(true)
end)

test("graph: a zoom is the view's alone, nothing settles again", function()
	local app = open(1440, 1400)
	view(app, "graph")
	app:frames(120)
	local before = app:state("control_map_graph_settle")
	expect(before.done).to_be(true)
	for _ = 1, 5 do
		app:command("map_graph_zoom_in")
		app:frames(2)
	end
	app:frames(5)
	local after = app:state("control_map_graph_settle")
	expect(after.runs).to_be(before.runs)
	expect(after.done).to_be(true)
end)

test("a machine renamed shows its new name at once, in every view", function()
	-- (Each view on screen when the rename lands: no rebuild, no restart.)
	for _, v in { "graph", "topology", "timeline", "machines" } do
		local app = open(1440, 1400)
		app:command("map_live")
		view(app, v)
		app:frames(if v == "graph" then 60 else 10)
		expect(string.find(app:tree("#control-map"), "DESKTOP-3NFV6JC", 1, true) ~= nil).to_be(true)
		app:command("fixture.rename_win")
		app:frames(5)
		local tree = app:tree("#control-map")
		expect(string.find(tree, "x16-win", 1, true) ~= nil).to_be(true)
		expect(string.find(tree, "DESKTOP-3NFV6JC", 1, true)).to_be(nil)
	end
end)

test("topology, Lines (the default): an orchestrator's rail runs down to its work on other machines", function()
	local app = open(1440, 1400)
	app:command("map_cross")
	view(app, "topology")
	app:frames(10)
	expect(app:state("control_map_topo_style")).to_be("lines")
	-- (No row names its origin; rails cross machines.)
	expect(app:has("#control-map >> .orch-origin")).to_be(false)
	local function spans(mode: string): (boolean, boolean)
		local l = Layout.layout(world(), 1200, mode)
		local across, edges = false, false
		for _, rail in l.rails do
			if (rail :: any).edge then
				edges = true
			else
				local lanes = {}
				for _, lane in l.lanes do
					if rail.top < lane.y + lane.h and rail.bottom > lane.y then
						lanes[lane.id] = true
					end
				end
				local n = 0
				for _ in lanes do
					n += 1
				end
				across = across or n > 1
			end
		end
		return across, edges
	end
	local across, edges = spans("lines")
	expect(across).to_be(true)
	expect(edges).to_be(false)
	across, edges = spans("tags")
	expect(across).to_be(false)
	expect(edges).to_be(true)
end)

test("topology, Tags: no line across machines; a row there carries an edge and names who started or leads it", function()
	local app = open(1440, 1400)
	app:command("map_cross")
	view(app, "topology")
	app:command("map_topo_tags")
	app:frames(10)
	expect(app:state("control_map_topo_style")).to_be("tags")
	expect(app:has('#control-map >> .orch-origin >> ~"Brain dump"')).to_be(true)
	expect(app:has('#control-map >> .orch-origin >> ~"Zeron fixes"')).to_be(true)
end)


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

test("topology: every branch ends where its row starts, at its middle, at any width", function()
	for _, w in WIDTHS do
		local l = Layout.layout(world(), w)
		local mids: { [string]: { number } } = {}
		for _, r in l.rows do
			expect(r.x).to_be(l.content_x)
			expect(r.x + r.w <= w).to_be(true)
			if r.orchestrator then
				mids[r.orchestrator] = mids[r.orchestrator] or {}
				table.insert(mids[r.orchestrator], r.mid)
			end
		end
		expect(#l.rails >= 4).to_be(true)
		for _, rail in l.rails do
			-- One branch for each of its rows, each on a row's middle.
			expect(#rail.branches).to_be(#mids[rail.orchestrator])
			for _, b in rail.branches do
				expect(b.to).to_be(l.content_x - Layout.GAP)
				expect(table.find(mids[rail.orchestrator], b.y) ~= nil).to_be(true)
			end
			-- Its lines: each branch's last point is that branch's end; the
			-- rail's own runs stay on its x, between its ends.
			local solid = Layout.lines(rail)
			local ends = 0
			for _, pts in solid do
				local n = #pts
				expect(pts[1]).to_be(rail.x)
				if pts[n - 1] == l.content_x - Layout.GAP then
					ends += 1
				else
					expect(pts[3]).to_be(rail.x)
					expect(pts[2] >= rail.top and pts[4] <= rail.bottom).to_be(true)
				end
			end
			expect(ends).to_be(#rail.branches)
			expect(rail.x < l.content_x - Layout.GAP - Layout.CORNER).to_be(true)
		end
		-- Rails that overlap never share a column.
		for _, a in l.rails do
			for _, b in l.rails do
				if a ~= b and a.x == b.x then
					expect(a.bottom < b.top or b.bottom < a.top).to_be(true)
				end
			end
		end
	end
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
	-- Five orchestrators, twelve crew, one of the person's own at work.
	expect(app:count("#control-widget-map >> .orch-node")).to_be(5)
	expect(app:count("#control-widget-map >> .orch-pill")).to_be(12)
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
	expect(app:count("#control-widget-map >> .orch-tl-lane")).to_be(18)
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
	app:resize(700, 2400)
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

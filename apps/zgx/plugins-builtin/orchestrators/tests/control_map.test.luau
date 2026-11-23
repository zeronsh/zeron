-- The Map (ds/orch_map) in the Control room: a lane per machine, stations
-- on their host's lane, crew pills on the lane they run on; a thread's
-- state reaching its pill alone; a click opening it. On
-- ui/control_page_fixture.luau.

local function open(): any
	local app = mount("control_page_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	-- (On the Map tab, in the Topology: still drawn, set once the tab is
	-- built, which opens a view it doesn't offer on the Graph.)
	app:set_state("control_tab", "map")
	app:frames(30)
	app:set_state("control_map", { variant = "topology", window = 15 })
	app:frames(30)
	return app
end

local function box(app: any, target: string): (number, number)
	local line = app:tree(target):match("[^\n]+")
	return tonumber(line:match(" (%-?%d+),%-?%d+ ")) :: number, tonumber(line:match(" %-?%d+,(%-?%d+) ")) :: number
end

test("each machine is a lane; crew sit on the lane they run on", function()
	local app = open()
	expect(app:count("#control-map >> .orch-lane")).to_be(3)
	expect(app:count("#control-map >> .orch-node")).to_be(4)
	expect(app:count("#control-map >> .orch-pill")).to_be(7)
	-- Orchestrators' station on desk; its Control room thread runs on the macbook, below.
	local _, station = box(app, '#control-map >> role:button "Orchestrators"')
	local _, mac_thread = box(app, '#control-map >> .orch-pill "Control room UI, Building"')
	expect(mac_thread > station).to_be(true)
end)

test("a thread's activity reaches its pill in place", function()
	local app = open()
	local before = app:tree('#control-map >> .orch-pill "Engine: reports + MCP tools, Editing"'):match("@(%d+)")
	app:command("fixture.activity")
	-- (Shown once its last state has had its 300 ms.)
	expect(app:wait('#control-map >> .orch-pill "Engine: reports + MCP tools, Merging"', { timeout_ms = 1500 })).to_be(true)
	local after = app:tree('#control-map >> .orch-pill "Engine: reports + MCP tools, Merging"'):match("@(%d+)")
	expect(after).to_be(before)
end)

test("a report runs a pulse along its line", function()
	local app = open()
	expect(app:count("#control-map >> .orch-pulse")).to_be(0)
	app:command("fixture.report")
	app:frames(2)
	expect(app:count("#control-map >> .orch-pulse")).to_be(1)
end)

test("a station opens its orchestrator's page", function()
	local app = open()
	app:click('#control-map >> role:button "Exnomic VPN"')
	app:frames(5)
	expect(app:state("shell_route_path")).to_be("/orchestrator/orc-vpn")
end)

test("the layout puts every line's end at a row, every row inside the width", function()
	local map = require("@orchestrators/ds/orch_map_layout")
	local data = require("@orchestrators/ds/orch_data")
	local sample = require("@orchestrators/account/orchestration_sample")
	local sessions_now = { chats = {}, children = {} }
	for _, c in sample.chats do
		table.insert(sessions_now.chats, c)
	end
	local l = map.layout({ now = sample.now, chats = data.chats(sessions_now), devices = sample.devices, loads = sample.loads, here = "dev-desk" }, 900)
	expect(#l.lanes).to_be(3)
	local mids: { [number]: boolean } = {}
	for _, r in l.rows do
		expect(r.x + r.w <= 900).to_be(true)
		mids[r.mid] = true
		if r.kind == "crew" then
			expect(l.routes[r.id] ~= nil).to_be(true)
		end
	end
	local xs: { [number]: number } = {}
	for _, r in l.rows do
		xs[r.mid] = r.x
	end
	for _, rail in l.rails do
		for _, b in rail.branches do
			expect(mids[b.y]).to_be(true)
			expect(b.to).to_be(xs[b.y] - map.GAP)
		end
	end
end)

test("the sample shifted to another time keeps its todo counts", function()
	local sample = require("@orchestrators/account/orchestration_sample")
	local later = sample.at(sample.NOW + 3600)
	for _, c in later.chats do
		if c.id == "th-ui-control" then
			expect(c.todo.done).to_be(2)
			expect(c.todo.total).to_be(7)
		end
	end
end)

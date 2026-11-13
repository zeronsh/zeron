-- The Map (ds/orch_map) in the Control room: a lane per machine, stations
-- on their host's lane, crew pills on the lane they run on; a thread's
-- state reaching its pill alone; a click opening it. On
-- ui/control_page_fixture.luau.

local function open(): any
	local app = mount("control_page_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	return app
end

local function box(app: any, target: string): (number, number)
	local line = app:tree(target):match("[^\n]+")
	return tonumber(line:match(" (%-?%d+),%-?%d+ ")) :: number, tonumber(line:match(" %-?%d+,(%-?%d+) ")) :: number
end

test("each machine is a lane; crew sit on the lane they run on", function()
	local app = open()
	expect(app:count("#control-widget-map >> .orch-lane")).to_be(3)
	expect(app:count("#control-widget-map >> .orch-node")).to_be(4)
	expect(app:count("#control-widget-map >> .orch-pill")).to_be(7)
	-- Orchestrators' station on desk; its Control room thread runs on the macbook, below.
	local _, station = box(app, '#control-widget-map >> role:button "Orchestrators"')
	local _, mac_thread = box(app, '#control-widget-map >> .orch-pill "Control room UI, Building"')
	expect(mac_thread > station).to_be(true)
end)

test("a thread's activity reaches its pill in place", function()
	local app = open()
	local before = app:tree('#control-widget-map >> .orch-pill "Engine: reports + MCP tools, Editing"'):match("@(%d+)")
	app:command("fixture.activity")
	app:frames(10)
	expect(app:has('#control-widget-map >> .orch-pill "Engine: reports + MCP tools, Merging"')).to_be(true)
	local after = app:tree('#control-widget-map >> .orch-pill "Engine: reports + MCP tools, Merging"'):match("@(%d+)")
	expect(after).to_be(before)
end)

test("a report runs a pulse along its line", function()
	local app = open()
	expect(app:count("#control-widget-map >> .orch-pulse")).to_be(0)
	app:command("fixture.report")
	app:frames(2)
	expect(app:count("#control-widget-map >> .orch-pulse")).to_be(1)
end)

test("a station opens its orchestrator's page", function()
	local app = open()
	app:click('#control-widget-map >> role:button "Exnomic VPN"')
	app:frames(5)
	expect(app:state("shell_route_path")).to_be("/orchestrator/orc-vpn")
end)

test("the layout puts every line's end at a station or a pill", function()
	local map = require("@ds/orch_map_layout")
	local data = require("@ds/orch_data")
	local sample = require("@account/orchestration_sample")
	local sessions_now = { chats = {}, children = {} }
	for _, c in sample.chats do
		table.insert(sessions_now.chats, c)
	end
	local l = map.layout({ now = sample.now, chats = data.chats(sessions_now), devices = sample.devices, loads = sample.loads, here = "dev-desk" }, 900)
	expect(#l.lanes).to_be(3)
	for _, p in l.pills do
		expect(l.routes[p.id] ~= nil).to_be(true)
		expect(p.x + p.w <= 900).to_be(true)
	end
end)

test("the sample shifted to another time keeps its todo counts", function()
	local sample = require("@account/orchestration_sample")
	local later = sample.at(sample.NOW + 3600)
	for _, c in later.chats do
		if c.id == "th-ui-control" then
			expect(c.todo.done).to_be(2)
			expect(c.todo.total).to_be(7)
		end
	end
end)

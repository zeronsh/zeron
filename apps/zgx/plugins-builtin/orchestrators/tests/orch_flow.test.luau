-- An orchestrator's flow (ds/orch_flow): its starting flows run as they
-- are; a version 2 team (Orchestrator Builder's JSON) opens, its icons and
-- providers left behind; what isn't one is refused with why; and the checks
-- find what would stop a flow running.

local flow = require("@orchestrators/ds/orch_flow")

local function keys(issues: { any }): { [string]: boolean }
	local out = {}
	for _, i in issues do
		out[i.key] = true
	end
	return out
end

-- A team as the plugin saves one: icons, a provider, its own next id.
local function team(): any
	return {
		version = 2,
		name = "Chat reuse verification",
		next_id = 9,
		nodes = {
			{ id = "1", kind = "chief", name = "Chief", x = 300, y = 48, instructions = "", harness = "", model = "" },
			{ id = "4", kind = "validator", name = "Validator", x = 300, y = 588, instructions = "Check it", harness = "codex", provider = "openai-codex", model = "openai-codex/gpt-6.1", icon = "<svg></svg>", icon_name = "v.svg" },
			{ id = "6", kind = "qa", name = "QA", x = 600, y = 768, instructions = "Try it", harness = "codex", model = "" },
			{ id = "5", kind = "finish", name = "Finish", x = 300, y = 768 },
		},
		edges = {
			{ from = "1", to = "4", condition = "always" },
			{ from = "4", to = "6", condition = "always" },
			{ from = "6", to = "4", condition = "fail", max_retries = 1 },
			{ from = "6", to = "5", condition = "pass" },
		},
	}
end

test("orch flow: Plan, build, review and Blank run as they are", function()
	expect(#flow.check(flow.initial())).to_be(0)
	expect(#flow.check(flow.blank())).to_be(0)
end)

test("orch flow: a version 2 team opens, only what a flow knows kept", function()
	local f, err = flow.import(team())
	expect(err).to_be(nil)
	expect(f ~= nil).to_be(true)
	local v = flow.node(f :: any, "4") :: any
	expect(v.model).to_be("openai-codex/gpt-6.1")
	expect(v.icon).to_be(nil)
	expect(v.provider).to_be(nil)
	expect((flow.node(f :: any, "5") :: any).instructions).to_be("")
	expect((f :: any).next_id).to_be(9)
	expect(#flow.check(f :: any)).to_be(0)
end)

test("orch flow: exported and imported again, the same flow", function()
	local f = flow.initial()
	local again = flow.import(flow.export(f)) :: any
	expect(again ~= nil).to_be(true)
	expect(again.name).to_be(f.name)
	expect(again.next_id).to_be(f.next_id)
	expect(#again.nodes).to_be(#f.nodes)
	for i, n in f.nodes do
		local m = again.nodes[i]
		expect(m.id .. m.kind .. m.name .. m.instructions .. m.x .. m.y).to_be(n.id .. n.kind .. n.name .. n.instructions .. n.x .. n.y)
	end
	for i, e in f.edges do
		local d = again.edges[i]
		expect(`{d.from}>{d.to}:{d.condition}:{d.max_retries}`).to_be(`{e.from}>{e.to}:{e.condition}:{e.max_retries}`)
	end
end)

test("orch flow: what isn't a flow is refused, saying why", function()
	local _, err = flow.import({ version = 1, name = "x", nodes = {}, edges = {} })
	expect(err).to_be("orch.flow.import.not_flow")
	local t = team()
	t.nodes[2].kind = "wizard"
	_, err = flow.import(t)
	expect(err).to_be("orch.flow.import.kind")
	t = team()
	t.edges[3].max_retries = 11
	_, err = flow.import(t)
	expect(err).to_be("orch.flow.issue.retries")
	t = team()
	table.insert(t.edges, { from = "1", to = "4", condition = "always" })
	_, err = flow.import(t)
	expect(err).to_be("orch.flow.import.duplicate")
	t = team()
	t.nodes[1].id = "2"
	_, err = flow.import(t)
	expect(err).to_be("orch.flow.import.entry")
end)

test("orch flow: a loop back on a fail needs a limit, and only a loop takes one", function()
	local f = flow.initial()
	for _, e in f.edges do
		if e.condition == "fail" then
			e.max_retries = nil
		end
	end
	local k = keys(flow.check(f))
	expect(k["orch.flow.issue.needs_limit"]).to_be(true)
	expect(k["orch.flow.issue.unbounded"]).to_be(true)

	local g = flow.blank()
	local n = flow.add(g, "qa", 0, 100) :: any
	g.edges = {
		{ from = "1", to = n.id, condition = "always" },
		{ from = n.id, to = "2", condition = "pass" },
	}
	local gate = flow.add(g, "gate", 200, 100) :: any
	table.insert(g.edges, { from = n.id, to = gate.id, condition = "fail", max_retries = 2 })
	table.insert(g.edges, { from = gate.id, to = "2", condition = "always" })
	expect(keys(flow.check(g))["orch.flow.issue.limit_forward"]).to_be(true)
end)

test("orch flow: the checks find a missing finish, a stray block and a bad route", function()
	local f = flow.initial()
	flow.remove(f, "5")
	local k = keys(flow.check(f))
	expect(k["orch.flow.issue.no_finish"]).to_be(true)
	expect(k["orch.flow.issue.outputs"]).to_be(true)

	local g = flow.initial()
	flow.add(g, "researcher", 400, 0)
	local k2 = keys(flow.check(g))
	expect(k2["orch.flow.issue.no_input"]).to_be(true)
	expect(k2["orch.flow.issue.unreached"]).to_be(true)
	expect(k2["orch.flow.issue.no_output"]).to_be(true)
end)

test("orch flow: nothing out of a finish, into the entry, or twice", function()
	local f = flow.initial()
	expect(flow.connect(f, "5", "2")).to_be(false)
	expect(flow.connect(f, "2", "1")).to_be(false)
	expect(flow.connect(f, "1", "2")).to_be(false)
	expect(flow.connect(f, "2", "2")).to_be(false)
	expect(flow.connect(f, "2", "4")).to_be(true)
end)

test("orch flow: a flow holds 32 blocks at most", function()
	local f = flow.blank()
	for _ = 1, 40 do
		flow.add(f, "worker", 0, 0)
	end
	expect(#f.nodes).to_be(flow.MAX_NODES)
end)

test("orch flow: its plan reads in order, with routes, limits and gates", function()
	local f = flow.initial()
	local gate = flow.add(f, "gate", 0, 0) :: any
	-- QA's pass goes to the gate, the gate on to the finish.
	for _, e in f.edges do
		if e.from == "6" and e.condition == "pass" then
			e.to = gate.id
		end
	end
	table.insert(f.edges, { from = gate.id, to = "5", condition = "always" })
	local plan = flow.plan(f)
	expect(string.find(plan, "## Flow plan", 1, true) ~= nil).to_be(true)
	local planner = string.find(plan, "1. **Planner**", 1, true)
	local worker = string.find(plan, "2. **Worker**", 1, true)
	local qa = string.find(plan, "4. **QA**", 1, true)
	expect(planner ~= nil and worker ~= nil and qa ~= nil).to_be(true)
	expect(string.find(plan, "On fail: back to step 3 (Validator), at most 3 times a run", 1, true) ~= nil).to_be(true)
	expect(string.find(plan, "On pass: step 5 (Gate)", 1, true) ~= nil).to_be(true)
	expect(string.find(plan, "**Gate** (ask the person)", 1, true) ~= nil).to_be(true)
	expect((string.find(plan, "{", 1, true))).to_be(nil)
end)

test("orch flow: the plan goes under notes already there, never over them", function()
	local f = flow.initial()
	expect(flow.with_plan("", f)).to_be(flow.plan(f))
	local both = flow.with_plan("Ship by Friday.", f)
	expect((string.find(both, "Ship by Friday.\n\n## Flow plan", 1, true))).to_be(1)
end)

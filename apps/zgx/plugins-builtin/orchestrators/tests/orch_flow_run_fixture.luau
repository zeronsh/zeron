-- A flow's run in the sample world (ui/orchestrator_fixture.luau): the
-- Orchestrators orchestrator holding Plan → Build → Review on the engine
-- (its `workflow`) and running it (its `flow_run`), at QA; its board the
-- run's four cards (Planner, Worker and Validator done, QA under way on a
-- thread), each waiting on the one before, and one card of no flow. For
-- tests/orch_flow_board.test.luau and pictures of the board and the Flow
-- view. The ops it's asked are noted as the page fixture notes them, and
-- answered as the engine would: a workflow sent is the orchestrator's
-- (and, in short, `orch_flow_sent`: its version, next id and blocks), a run stopped is stopped, one started
-- runs from its first step. Commands:
--   orch.dark / orch.light      the theme (the page fixture's)
--   view.work / view.flow       the page's Work or Flow
--   run.none                    no run (Run… offered)
--   run.blocked                 the run waiting on the person, at QA

local shell = require("./orchestrator_fixture")
local flow = require("@orchestrators/ds/orch_flow")
local orchestration = require("@orchestrators/account/orchestration")
local page = require("@orchestrators/screens/orchestrator/page")

local ID = "orc-orch"
local RUN = "run-1"
local NOW = os.time()
local workflow = flow.export(flow.initial())

-- The run's steps, in order: its node, kind, name and card.
local STEPS = {
	{ node = "2", kind = "planner", step = "Planner", card = "it-f-plan", title = "Plan: flow runs on the board" },
	{ node = "3", kind = "worker", step = "Worker", card = "it-f-build", title = "Build: flow runs on the board" },
	{ node = "4", kind = "validator", step = "Validator", card = "it-f-check", title = "Validate: flow runs on the board" },
	{ node = "6", kind = "qa", step = "QA", card = "it-f-qa", title = "QA: flow runs on the board" },
}

local function run(state: string, at: string?): any
	local cards = {}
	for _, s in STEPS do
		cards[s.node] = s.card
	end
	return {
		id = RUN,
		flow = workflow,
		task = "Show flow runs on the board",
		state = state,
		cards = cards,
		returns = {},
		at = at,
		started = NOW - 3600,
	}
end

local function items(): { any }
	local out: { any } = {}
	local before: string? = nil
	for i, s in STEPS do
		local last = i == #STEPS
		table.insert(out, {
			id = s.card,
			orchestrator_id = ID,
			title = s.title,
			detail = "",
			state = if last then "active" else "done",
			priority = "normal",
			threads = if last then { "th-engine-flow" } else {},
			links = {},
			order = i,
			created = NOW - 3600 + i * 600,
			-- (Done moments ago: today's, whatever the hour.)
			updated = NOW - (#STEPS - i) * 5,
			done = if last then nil else NOW - (#STEPS - i) * 5,
			outcome = if last then nil else `{s.step} done.`,
			flow = { run = RUN, node = s.node, kind = s.kind, flow = workflow.name, step = s.step, index = i, of = #STEPS },
			depends_on = if before then { before } else {},
		})
		before = s.card
	end
	table.insert(out, {
		id = "it-live",
		orchestrator_id = ID,
		title = "Live end-to-end test on a dev engine",
		detail = "",
		state = "planned",
		priority = "normal",
		threads = {},
		links = {},
		order = 9,
		created = NOW - 5300,
		updated = NOW - 5300,
	})
	return out
end

-- Orchestrators' row changed by `fn` (on a copy), the rest as they are.
local function with_orch(fn: (o: any) -> ())
	local now = table.clone(orchestration.now:peek())
	local list = {}
	for _, o in now.orchestrators or {} do
		if o.id == ID then
			o = table.clone(o)
			fn(o)
		end
		table.insert(list, o)
	end
	now.orchestrators = list
	orchestration.now:set(now)
end

do
	local now = table.clone(orchestration.now:peek())
	local list = {}
	for _, it in now.items or {} do
		if it.orchestrator_id ~= ID then
			table.insert(list, it)
		end
	end
	for _, it in items() do
		table.insert(list, it)
	end
	now.items = list
	orchestration.now:set(now)
end
with_orch(function(o: any)
	o.workflow = workflow
	o.flow_run = run("running", "6")
end)

local sent = ui.state("orch_flow_sent", "")
local base_op = (orchestration :: any).op
;(orchestration :: any).op = function(op: { [string]: any }): any
	local reply = base_op(op)
	if op.op == "updateOrchestrator" and op.id == ID and op.workflow then
		local w: any = op.workflow
		local names = {}
		for _, n in w.nodes or {} do
			table.insert(names, `{n.id}={n.name}`)
		end
		sent:set(`version={w.version};next_id={w.next_id};{table.concat(names, ",")}`)
		with_orch(function(o: any)
			o.workflow = flow.copy(op.workflow)
		end)
	elseif op.op == "stopFlow" and op.orchestratorId == ID then
		with_orch(function(o: any)
			if o.flow_run then
				o.flow_run = table.clone(o.flow_run)
				o.flow_run.state = "stopped"
			end
		end)
	elseif op.op == "startFlow" and op.orchestratorId == ID then
		with_orch(function(o: any)
			o.flow_run = run("running", "2")
			o.flow_run.task = op.task
		end)
	end
	return reply
end

page.view:set("work")
require("@orchestrators/ds/orch_flow_editor").selected:set({})

ui.command { id = "view.work", title = "Work", run = function()
	page.view:set("work")
end }
ui.command { id = "view.flow", title = "Flow", run = function()
	page.view:set("flow")
end }
ui.command { id = "run.none", title = "No run", run = function()
	with_orch(function(o: any)
		o.flow_run = nil
	end)
end }
ui.command { id = "run.blocked", title = "Waiting on you", run = function()
	with_orch(function(o: any)
		o.flow_run = run("blocked", "6")
	end)
end }

return shell

-- An orchestrator's flow: the steps its crew works through and how one
-- leads to the next. A flow is blocks (an entry, roles, gates, a finish)
-- joined by connections; a connection always follows, or follows only on
-- a pass or a fail, and a fail that goes back to an earlier step is
-- limited to a number of returns a run.
--
-- The shape is the "version 2 team" JSON of Orchestrator Builder, the
-- Zeron plugin by Master0fFate (github.com/Master0fFate/zeron-orchestration-
-- builder), so a team made there opens here; written anew for Zeron, with
-- a Gate block (a person's approval) beside its roles.
--
--   flow.initial()                 Plan → Build → Review
--   flow.blank()                   only the entry and a finish
--   flow.import(value)             a decoded JSON value checked and made a
--                                  flow, or nil and why
--   flow.export(f)                 its JSON value (encode it to save)
--   flow.check(f)                  what's wrong with it: { { id?, key } }
--   flow.add(f, kind, x, y)        a block added (nil when full)
--   flow.connect(f, from, to)      a connection added (false when it can't)
--   flow.remove(f, id)             a block and its connections removed
--   flow.node(f, id)               a block by id
--   flow.copy(v)                   a deep copy
--   flow.KINDS, flow.ROLES         the block kinds, the ones added by hand
--   flow.instructions(kind)        a role's starting instructions

local M = {}

export type Node = {
	id: string,
	kind: string,
	name: string,
	x: number,
	y: number,
	instructions: string,
	harness: string,
	model: string,
}

export type Edge = {
	from: string,
	to: string,
	condition: string,
	max_retries: number?,
}

export type Flow = {
	version: number,
	name: string,
	next_id: number,
	nodes: { Node },
	edges: { Edge },
}

export type Issue = { id: string?, key: string }

M.MAX_NODES = 32
M.MAX_EDGES = 1024
M.MAX_RETRIES = 10
local MAX_TEXT = 100000
local MAX_NAME = 256

export type Kind = { name: string, icon: string, instructions: string }

-- Each kind: its starting name, its icon, and what its role is asked to do.
M.KINDS = ({
	chief = { name = "Entry", icon = "action-play", instructions = "" },
	planner = {
		name = "Planner",
		icon = "checklist",
		instructions = "Read the task and what it touches. Set out acceptance criteria, dependencies and risks, then an ordered plan short enough to follow. Change nothing; say what's unclear.",
	},
	worker = {
		name = "Worker",
		icon = "file-code",
		instructions = "Do the planned work, in scope, keeping changes that are already there. Run the checks the project has. Say what changed, how you know it works, and what's in the way.",
	},
	validator = {
		name = "Validator",
		icon = "check-circle",
		instructions = "Check the work against the task and its acceptance criteria on your own: correctness, regressions, evidence. When it comes back from review, answer what review found. Pass when your check is complete; fail only when you couldn't do it.",
	},
	auditor = {
		name = "Auditor",
		icon = "shield-check",
		instructions = "Look for correctness, security and operational risk in the work. Tie each finding to evidence, say what it costs and the smallest fix. Change nothing; say where you're unsure.",
	},
	qa = {
		name = "QA",
		icon = "action-test",
		instructions = "Try the work the way it will be used, edge and failure cases included. Pass when it holds; fail to send it back with what to fix. Say what you ran and what you didn't.",
	},
	researcher = {
		name = "Researcher",
		icon = "magnifer",
		instructions = "Answer the question from primary sources and the project itself. Keep facts apart from guesses, cite where each came from, and say what's still unknown.",
	},
	gate = {
		name = "Gate",
		icon = "status-approval",
		instructions = "Ask the person to approve before going on: what was done, what's next, and what to look at.",
	},
	finish = { name = "Finish", icon = "flag", instructions = "" },
} :: any) :: { [string]: Kind }

-- The kinds added by hand, in the order offered.
M.ROLES = { "planner", "worker", "validator", "auditor", "qa", "researcher", "gate", "finish" }

local CONDITIONS = { always = true, pass = true, fail = true }

function M.instructions(kind: string): string
	local k = M.KINDS[kind]
	return if k then k.instructions else ""
end

function M.copy<T>(value: T): T
	if type(value) ~= "table" then
		return value :: any
	end
	local out = {}
	for k, v in value :: any do
		out[k] = M.copy(v)
	end
	return out :: any
end

local function has_text(s: any): boolean
	return type(s) == "string" and string.find(s, "%S") ~= nil
end

local function make(id: string, kind: string, x: number, y: number): Node
	local k = M.KINDS[kind]
	return { id = id, kind = kind, name = k.name, x = x, y = y, instructions = k.instructions, harness = "", model = "" }
end

function M.blank(): Flow
	return {
		version = 2,
		name = "Blank",
		next_id = 3,
		nodes = { make("1", "chief", 0, 0), make("2", "finish", 0, 200) },
		edges = { { from = "1", to = "2", condition = "always" } },
	}
end

-- Plan, build, then review: QA sends it back to the Validator on a fail
-- (three times at most), on to the Finish on a pass.
function M.initial(): Flow
	return {
		version = 2,
		name = "Plan → Build → Review",
		next_id = 7,
		nodes = {
			make("1", "chief", 0, 0),
			make("2", "planner", 0, 160),
			make("3", "worker", 0, 320),
			make("4", "validator", 0, 480),
			make("6", "qa", 0, 640),
			make("5", "finish", 0, 800),
		},
		edges = {
			{ from = "1", to = "2", condition = "always" },
			{ from = "2", to = "3", condition = "always" },
			{ from = "3", to = "4", condition = "always" },
			{ from = "4", to = "6", condition = "always" },
			{ from = "6", to = "4", condition = "fail", max_retries = 3 },
			{ from = "6", to = "5", condition = "pass" },
		},
	}
end

function M.node(f: Flow, id: string): Node?
	for _, n in f.nodes do
		if n.id == id then
			return n
		end
	end
	return nil
end

function M.add(f: Flow, kind: string, x: number, y: number): Node?
	if not M.KINDS[kind] or kind == "chief" or #f.nodes >= M.MAX_NODES then
		return nil
	end
	local n = make(tostring(f.next_id), kind, x, y)
	f.next_id += 1
	table.insert(f.nodes, n)
	return n
end

-- Whether a connection from `from` to `to` may be drawn: two blocks, not the
-- same, nothing out of a finish or into the entry, not there already.
function M.can_connect(f: Flow, from: string, to: string): boolean
	local a, b = M.node(f, from), M.node(f, to)
	if not a or not b or from == to or a.kind == "finish" or b.kind == "chief" or #f.edges >= M.MAX_EDGES then
		return false
	end
	for _, e in f.edges do
		if e.from == from and e.to == to then
			return false
		end
	end
	return true
end

function M.connect(f: Flow, from: string, to: string): boolean
	if not M.can_connect(f, from, to) then
		return false
	end
	table.insert(f.edges, { from = from, to = to, condition = "always" })
	return true
end

function M.remove(f: Flow, id: string)
	local n = M.node(f, id)
	if not n or n.kind == "chief" then
		return
	end
	for i = #f.nodes, 1, -1 do
		if f.nodes[i].id == id then
			table.remove(f.nodes, i)
		end
	end
	for i = #f.edges, 1, -1 do
		if f.edges[i].from == id or f.edges[i].to == id then
			table.remove(f.edges, i)
		end
	end
end

-- The flow as a plan to read and follow (an orchestrator's notes): its
-- steps in order from the entry, each with its agent and model, where it
-- goes on a pass or a fail and how often a fail may go back; a gate asks
-- the person. Markdown, under a "Flow plan" heading.
function M.plan(f: Flow): string
	local outs: { [string]: { Edge } } = {}
	for _, e in f.edges do
		outs[e.from] = outs[e.from] or {}
		table.insert(outs[e.from], e)
	end
	-- In order: the way on (always, then pass) before a fail's way back.
	local rank = { always = 1, pass = 2, fail = 3 }
	local order: { Node } = {}
	local seen: { [string]: boolean } = {}
	local queue = { "1" }
	while #queue > 0 do
		local id = table.remove(queue, 1) :: string
		local n = M.node(f, id)
		if n and not seen[id] then
			seen[id] = true
			table.insert(order, n)
			local list = table.clone(outs[id] or {})
			table.sort(list, function(a: Edge, b: Edge): boolean
				return (rank[a.condition] or 9) < (rank[b.condition] or 9)
			end)
			for _, e in list do
				table.insert(queue, e.to)
			end
		end
	end
	local number: { [string]: number } = {}
	local steps: { Node } = {}
	for _, n in order do
		if n.kind ~= "chief" then
			table.insert(steps, n)
			number[n.id] = #steps
		end
	end
	local function ref(id: string): string
		local n = M.node(f, id)
		if not n then
			return "?"
		end
		if n.kind == "finish" then
			return "finish"
		end
		return `step {number[id] or "?"} ({n.name})`
	end
	local lines = { "## Flow plan", "", `{f.name}: follow these steps in order, one crew thread a step.`, "" }
	for i, n in steps do
		if n.kind == "finish" then
			table.insert(lines, `{i}. **{n.name}**: done.`)
			continue
		end
		local who = if n.kind == "gate"
			then "ask the person"
			elseif n.model ~= "" then (if n.harness ~= "" then `{n.harness} · {n.model}` else n.model)
			elseif n.harness ~= "" then n.harness
			else "crew model"
		local line = `{i}. **{n.name}** ({who})`
		local brief = string.match(n.instructions, "^%s*(.-)%s*$") or ""
		if brief ~= "" then
			line ..= `: {string.gsub(brief, "%s*\n%s*", " ")}`
		end
		table.insert(lines, line)
		for _, e in outs[n.id] or {} do
			if e.condition == "pass" then
				table.insert(lines, `   - On pass: {ref(e.to)}.`)
			elseif e.condition == "fail" then
				local limit = if e.max_retries then `, at most {e.max_retries} time{if e.max_retries == 1 then "" else "s"} a run; then stop and ask the person` else ""
				table.insert(lines, `   - On fail: back to {ref(e.to)}{limit}.`)
			elseif number[e.to] ~= i + 1 then
				table.insert(lines, `   - Then: {ref(e.to)}.`)
			end
		end
	end
	return table.concat(lines, "\n")
end

-- Notes with the plan added: under what's there, never in its place.
function M.with_plan(notes: string?, f: Flow): string
	local had = string.match(notes or "", "^%s*(.-)%s*$") or ""
	local plan = M.plan(f)
	return if had == "" then plan else `{had}\n\n{plan}`
end

-- What's wrong with one connection's settings, a locale key, or nil.
local function edge_fault(e: any): string?
	local c = e.condition
	if c ~= nil and not CONDITIONS[c] then
		return "orch.flow.issue.condition"
	end
	local r = e.max_retries
	if r ~= nil then
		if c ~= "fail" or type(r) ~= "number" or r ~= math.floor(r) or r < 1 or r > M.MAX_RETRIES then
			return "orch.flow.issue.retries"
		end
	end
	return nil
end

-- A decoded JSON value as a flow: the shape checked and only known fields
-- kept (a team's icons and providers are left behind). Nil and a locale key
-- when it isn't one.
function M.import(value: any): (Flow?, string?)
	if type(value) ~= "table" or value.version ~= 2 or not has_text(value.name) or #value.name > MAX_NAME
		or type(value.nodes) ~= "table" or type(value.edges) ~= "table" then
		return nil, "orch.flow.import.not_flow"
	end
	local out: Flow = { version = 2, name = value.name, next_id = 2, nodes = {}, edges = {} }
	local kinds: { [string]: string } = {}
	local top, entry = 1, false
	for i, n in value.nodes do
		if type(i) ~= "number" or i ~= #out.nodes + 1 or i > M.MAX_NODES or type(n) ~= "table" then
			return nil, "orch.flow.import.too_many"
		end
		local num = if type(n.id) == "string" and string.match(n.id, "^[1-9]%d*$") then tonumber(n.id) else nil
		if not num or num > 1e9 or kinds[n.id] then
			return nil, "orch.flow.import.ids"
		end
		if not M.KINDS[n.kind] then
			return nil, "orch.flow.import.kind"
		end
		if n.kind == "chief" then
			if n.id ~= "1" or entry then
				return nil, "orch.flow.import.entry"
			end
			entry = true
		end
		if type(n.name) ~= "string" or #n.name > MAX_NAME or type(n.x) ~= "number" or type(n.y) ~= "number"
			or n.x ~= n.x or n.y ~= n.y or math.abs(n.x) > 1e6 or math.abs(n.y) > 1e6 then
			return nil, "orch.flow.import.block"
		end
		for _, field in { "instructions", "harness", "model" } do
			if n[field] ~= nil and (type(n[field]) ~= "string" or #n[field] > MAX_TEXT) then
				return nil, "orch.flow.import.block"
			end
		end
		kinds[n.id] = n.kind
		top = math.max(top, num)
		table.insert(out.nodes, {
			id = n.id,
			kind = n.kind,
			name = n.name,
			x = n.x,
			y = n.y,
			instructions = n.instructions or "",
			harness = n.harness or "",
			model = n.model or "",
		})
	end
	if not entry then
		return nil, "orch.flow.import.entry"
	end
	local seen: { [string]: boolean } = {}
	for i, e in value.edges do
		if type(i) ~= "number" or i ~= #out.edges + 1 or i > M.MAX_EDGES or type(e) ~= "table"
			or type(e.from) ~= "string" or type(e.to) ~= "string" or not kinds[e.from] or not kinds[e.to]
			or e.from == e.to or kinds[e.from] == "finish" or kinds[e.to] == "chief" then
			return nil, "orch.flow.import.connection"
		end
		local fault = edge_fault(e)
		if fault then
			return nil, fault
		end
		local key = e.from .. ">" .. e.to
		if seen[key] then
			return nil, "orch.flow.import.duplicate"
		end
		seen[key] = true
		table.insert(out.edges, { from = e.from, to = e.to, condition = e.condition or "always", max_retries = e.max_retries })
	end
	local next_id = value.next_id
	out.next_id = if type(next_id) == "number" and next_id == math.floor(next_id) and next_id > top and next_id <= 1e9 + 1
		then next_id
		else top + 1
	return out, nil
end

-- Its JSON value, as the version 2 team shape (fields in a fixed set).
function M.export(f: Flow): any
	local nodes = {}
	for _, n in f.nodes do
		table.insert(nodes, {
			id = n.id,
			kind = n.kind,
			name = n.name,
			x = n.x,
			y = n.y,
			instructions = n.instructions,
			harness = n.harness,
			model = n.model,
		})
	end
	local edges = {}
	for _, e in f.edges do
		table.insert(edges, { from = e.from, to = e.to, condition = e.condition, max_retries = e.max_retries })
	end
	return { version = 2, name = f.name, next_id = f.next_id, nodes = nodes, edges = edges }
end

-- Everything that would stop it running, each a locale key and the block it
-- is about (nil: the flow as a whole). Empty: it can run.
function M.check(f: Flow): { Issue }
	local issues: { Issue } = {}
	local function say(id: string?, key: string)
		table.insert(issues, { id = id, key = key })
	end
	local nodes: { [string]: Node } = {}
	local ins: { [string]: number } = {}
	local outs: { [string]: { Edge } } = {}
	local entry: Node? = nil
	for _, n in f.nodes do
		if nodes[n.id] then
			say(n.id, "orch.flow.issue.duplicate_id")
		end
		nodes[n.id], ins[n.id], outs[n.id] = n, 0, {}
		if n.kind == "chief" then
			if entry then
				say(n.id, "orch.flow.issue.one_entry")
			else
				entry = n
			end
		end
	end
	local seen: { [string]: boolean } = {}
	for _, e in f.edges do
		if not nodes[e.from] or not nodes[e.to] then
			say(nil, "orch.flow.issue.dangling")
			continue
		end
		local key = e.from .. ">" .. e.to
		if seen[key] then
			say(e.from, "orch.flow.issue.duplicate")
		end
		seen[key] = true
		if e.from == e.to or nodes[e.from].kind == "finish" or nodes[e.to].kind == "chief" then
			say(e.from, "orch.flow.issue.endpoints")
		end
		local fault = edge_fault(e)
		if fault then
			say(e.from, fault)
		end
		if e.condition == "fail" and nodes[e.to].kind == "finish" then
			say(e.from, "orch.flow.issue.fail_to_finish")
		end
		table.insert(outs[e.from], e)
		ins[e.to] += 1
	end
	-- Whether `to` is reached from `from` without crossing `skip`.
	local function reaches(from: string, to: string, skip: Edge): boolean
		local been: { [string]: boolean } = {}
		local function go(id: string): boolean
			if id == to then
				return true
			end
			if been[id] then
				return false
			end
			been[id] = true
			for _, e in outs[id] or {} do
				if e ~= skip and go(e.to) then
					return true
				end
			end
			return false
		end
		return go(from)
	end
	-- A fail that goes back is limited; a limit only on one that goes back.
	for _, list in outs do
		for _, e in list do
			if e.condition == "fail" then
				local back = reaches(e.to, e.from, e)
				if back and e.max_retries == nil then
					say(e.from, "orch.flow.issue.needs_limit")
				elseif e.max_retries ~= nil and not back then
					say(e.from, "orch.flow.issue.limit_forward")
				end
			end
		end
	end
	-- Every loop has a limited return on it.
	local done: { [string]: boolean } = {}
	local open: { [string]: boolean } = {}
	local function walk(id: string)
		if open[id] then
			say(id, "orch.flow.issue.unbounded")
			return
		end
		if done[id] then
			return
		end
		open[id], done[id] = true, true
		for _, e in outs[id] do
			if not (e.condition == "fail" and e.max_retries ~= nil and not edge_fault(e)) then
				walk(e.to)
			end
		end
		open[id] = nil
	end
	for _, n in f.nodes do
		walk(n.id)
	end
	local reached: { [string]: boolean } = {}
	local function mark(id: string)
		if reached[id] then
			return
		end
		reached[id] = true
		for _, e in outs[id] do
			mark(e.to)
		end
	end
	if entry then
		mark(entry.id)
	else
		say(nil, "orch.flow.issue.no_entry")
	end
	local finish = false
	for _, n in f.nodes do
		if not has_text(n.name) then
			say(n.id, "orch.flow.issue.name")
		end
		if n.kind ~= "chief" and ins[n.id] == 0 then
			say(n.id, "orch.flow.issue.no_input")
		end
		local list = outs[n.id]
		if n.kind ~= "finish" then
			if #list == 0 then
				say(n.id, "orch.flow.issue.no_output")
			elseif #list == 1 then
				if list[1].condition ~= "always" then
					say(n.id, "orch.flow.issue.outputs")
				end
			else
				local pass, fail = 0, 0
				for _, e in list do
					if e.condition == "pass" then
						pass += 1
					elseif e.condition == "fail" then
						fail += 1
					end
				end
				if n.kind == "chief" or #list ~= 2 or pass ~= 1 or fail ~= 1 then
					say(n.id, "orch.flow.issue.outputs")
				end
			end
		end
		if not reached[n.id] then
			say(n.id, "orch.flow.issue.unreached")
		end
		if n.kind ~= "chief" and n.kind ~= "finish" and not has_text(n.instructions) then
			say(n.id, "orch.flow.issue.instructions")
		end
		if n.model ~= "" and n.harness == "" then
			say(n.id, "orch.flow.issue.agent")
		end
		if n.kind == "finish" and reached[n.id] then
			finish = true
		end
	end
	if not finish then
		say(nil, "orch.flow.issue.no_finish")
	end
	return issues
end

return M

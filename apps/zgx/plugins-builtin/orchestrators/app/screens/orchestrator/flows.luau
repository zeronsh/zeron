-- Orchestrators' flows (ds/orch_flow): each orchestrator's own, as the
-- engine holds it (its `workflow`), else as kept on this device (from
-- before the engine held them), and the flows saved as templates to start
-- new ones from. Files in and out are the version 2 team JSON
-- (Orchestrator Builder's, by Master0fFate).
--
--   flows.of(id)                 orchestrator `id`'s flow, or nil (tracked):
--                                an edit not yet in what the engine says,
--                                else the engine's, else this device's
--   flows.set(id, f)             its flow kept here and sent to the engine
--                                (`updateOrchestrator { workflow }`) once
--                                the edits rest
--   flows.flush(id)              its last edit sent now (a run about to
--                                start)
--   flows.forget(id)             this device's copy of it let go
--   flows.starters()             what a new one can start from: { { value,
--                                label, flow? } } (tracked)
--   flows.starter(value)         the flow a starter's value names, a copy
--   flows.save_template(f)       a copy kept as a template (same name: replaced)
--   flows.remove_template(name)
--   flows.import_file(done)      done(flow?, error key?) from a file the
--                                person picks
--   flows.export_file(f, done)   done(saved, error key?), where they pick

local flow = require("../../ds/orch_flow")
local locale = require("@ds/locale")
local orchestration = require("../../account/orchestration")
local rows = require("../../account/rows")
local toast = require("@ds/toast")

local M = {}

M.BLANK = "blank"
M.DEFAULT = "default"
local TEMPLATE = "template:"
M.IMPORTED = "imported"

-- (Each orchestrator's flow, by its id; the templates, in the order saved.)
M.saved = ui.persist("orch_flows", {} :: { [string]: any })
M.templates = ui.persist("orch_flow_templates", {} :: { any })

-- Files bigger than this aren't a flow.
local MAX_FILE = 500000

-- Edits made here the engine doesn't say yet, by id: shown over its
-- `workflow` until it does (a drag doesn't jump back while it's on its way).
local pending = ui.state("orch_flow_pending", {} :: { [string]: any })
-- Sent once the edits rest this long (a drag is many).
local SEND_AFTER = 600
-- And let go this long after the engine took one, whatever it says then.
local SETTLE_AFTER = 3000
local edits: { [string]: number } = {}
-- The last edit sent, by id.
local sent: { [string]: number } = {}

-- The engine's workflow as a flow (nil: none, or not one), made once a
-- table the host gives.
local made: { [any]: any } = setmetatable({}, { __mode = "k" }) :: any
local function engine_flow(id: string): any
	local o = rows.orchestrator(id)
	local w = o and o.workflow
	if type(w) ~= "table" then
		return nil
	end
	local f = made[w]
	if f == nil then
		f = flow.import(w) or false
		made[w] = f
	end
	return f or nil
end

local function same(a: any, b: any): boolean
	if type(a) ~= "table" or type(b) ~= "table" then
		return a == b
	end
	for k, v in a do
		if not same(v, b[k]) then
			return false
		end
	end
	for k in b do
		if a[k] == nil then
			return false
		end
	end
	return true
end

local function settle(id: string)
	local all = pending:peek()
	if all[id] == nil then
		return
	end
	local left = table.clone(all)
	left[id] = nil
	pending:set(left)
end

-- (The engine saying what was sent puts the edit to rest.)
ui.effect(function()
	local all = pending:get()
	for id, f in all do
		local e = engine_flow(id)
		if e and same(flow.export(e), flow.export(f)) then
			ui.after(0, function()
				settle(id)
			end)
		end
	end
end)

function M.of(id: string?): any
	if not id then
		return nil
	end
	local mine = pending:get()[id]
	if mine then
		return mine
	end
	return engine_flow(id) or M.saved:get()[id]
end

local function send(id: string, n: number)
	if edits[id] ~= n or sent[id] == n then
		return
	end
	local f = pending:peek()[id]
	if not f then
		return
	end
	sent[id] = n
	orchestration.op({ op = "updateOrchestrator", id = id, workflow = flow.export(f) }):after(function()
		ui.after(SETTLE_AFTER, function()
			if edits[id] == n then
				settle(id)
			end
		end)
	end, function()
		-- (Kept here: shown as it is, sent again with the next edit.)
		toast.show { title = locale.s("orch.flow.sync_failed"), tone = "danger" }
	end)
end

function M.set(id: string, f: any)
	local all = flow.copy(M.saved:peek())
	all[id] = flow.copy(f)
	local mine = table.clone(pending:peek())
	mine[id] = flow.copy(f)
	ui.batch(function()
		M.saved:set(all)
		pending:set(mine)
	end)
	local n = (edits[id] or 0) + 1
	edits[id] = n
	ui.after(SEND_AFTER, function()
		send(id, n)
	end)
end

-- (Edits from before a reload, not yet taken: sent again.)
for id in pending:peek() do
	edits[id] = 1
	ui.after(SEND_AFTER, function()
		send(id, 1)
	end)
end

-- Its last edit sent now, not once the edits rest (before a run starts).
function M.flush(id: string)
	local n = edits[id]
	if n then
		send(id, n)
	end
end

function M.forget(id: string)
	edits[id] = (edits[id] or 0) + 1
	local all = flow.copy(M.saved:peek())
	all[id] = nil
	ui.batch(function()
		M.saved:set(all)
		settle(id)
	end)
end

function M.starters(): { any }
	local out: { any } = {
		{ value = M.BLANK, label = locale.s("orch.flow.blank") },
		{ value = M.DEFAULT, label = flow.initial().name },
	}
	for _, t in M.templates:get() do
		table.insert(out, { value = TEMPLATE .. t.name, label = t.name })
	end
	return out
end

function M.starter(value: string): any
	if value == M.BLANK then
		return nil
	elseif value == M.DEFAULT then
		return flow.initial()
	end
	local name = string.match(value, "^template:(.*)$")
	if name then
		for _, t in M.templates:peek() do
			if t.name == name then
				return flow.copy(t)
			end
		end
	end
	return nil
end

function M.save_template(f: any)
	local list = flow.copy(M.templates:peek())
	for i = #list, 1, -1 do
		if list[i].name == f.name then
			table.remove(list, i)
		end
	end
	table.insert(list, flow.copy(f))
	M.templates:set(list)
end

function M.remove_template(name: string)
	local list = flow.copy(M.templates:peek())
	for i = #list, 1, -1 do
		if list[i].name == name then
			table.remove(list, i)
		end
	end
	M.templates:set(list)
end

-- A file's text as a flow, or nil and a locale key.
function M.parse(text: string): (any, string?)
	if #text > MAX_FILE then
		return nil, "orch.flow.import.too_big"
	end
	local ok, value = pcall(ui.json.decode, text)
	if not ok then
		return nil, "orch.flow.import.not_json"
	end
	return flow.import(value)
end

-- A flow from a file the person picks: `done(flow?, error key?)`; nothing
-- picked, both nil.
function M.import_file(done: (any, string?) -> ())
	ui.task(function()
		local ok, files = pcall(function(): any
			return ui.open_files({ title = ui.t("orch.flow.import.title"):get(), filters = { { "JSON", { "json" } } } }):await()
		end)
		local file = ok and files and files[1]
		if not file then
			done(nil, if ok then nil else "orch.flow.import.unreadable")
			return
		end
		if (file.size or 0) > MAX_FILE then
			done(nil, "orch.flow.import.too_big")
			return
		end
		local read, text = pcall(function(): any
			return ui.read_file(file):await()
		end)
		if not read or type(text) ~= "string" then
			done(nil, "orch.flow.import.unreadable")
			return
		end
		done(M.parse(text))
	end)
end

-- Saved where the person picks: `done(saved, error key?)`.
function M.export_file(f: any, done: (boolean, string?) -> ())
	ui.task(function()
		local name = string.gsub(f.name, '[<>:"/\\|%?%*%c]', "_")
		local ok, why = pcall(function(): boolean
			local file = ui.save_file({ title = ui.t("orch.flow.export.title"):get(), name = `{name}.json`, filters = { { "JSON", { "json" } } } }):await()
			if not file then
				return false
			end
			ui.write_file(file, ui.json.encode(flow.export(f), { pretty = true })):await()
			return true
		end)
		if ok then
			done(why, nil)
		else
			done(false, "orch.flow.export.failed")
		end
	end)
end

return M

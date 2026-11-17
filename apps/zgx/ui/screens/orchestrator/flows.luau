-- Orchestrators' flows (ds/orch_flow), kept on this device until the
-- engine holds them: each orchestrator's own, by its id, and the flows
-- saved as templates to start new ones from. Files in and out are the
-- version 2 team JSON (Orchestrator Builder's, by Master0fFate).
--
--   flows.of(id)                 orchestrator `id`'s flow, or nil (tracked)
--   flows.set(id, f)             its flow kept
--   flows.starters()             what a new one can start from: { { value,
--                                label, flow? } } (tracked)
--   flows.starter(value)         the flow a starter's value names, a copy
--   flows.save_template(f)       a copy kept as a template (same name: replaced)
--   flows.remove_template(name)
--   flows.import_file(done)      done(flow?, error key?) from a file the
--                                person picks
--   flows.export_file(f, done)   done(saved, error key?), where they pick

local flow = require("@ds/orch_flow")
local locale = require("@ds/locale")

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

function M.of(id: string?): any
	if not id then
		return nil
	end
	return M.saved:get()[id]
end

function M.set(id: string, f: any)
	local all = flow.copy(M.saved:peek())
	all[id] = flow.copy(f)
	M.saved:set(all)
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

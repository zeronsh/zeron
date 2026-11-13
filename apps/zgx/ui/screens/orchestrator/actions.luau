-- What the person does to an orchestrator, from wherever it's asked (the
-- sidebar's row and its "+", a chat's menu, the palette, its page): each
-- one `Orchestrate` op (@account/orchestration), a word when it didn't go
-- through. The dialogs themselves are screens/orchestrator/dialogs.
--
--   actions.open(id)              to its page
--   actions.new()                 the New orchestrator dialog
--   actions.settings(id)          its settings dialog
--   actions.set_paused(id, on)    paused (no new crew work, no wakes) or not
--   actions.archive(id)           archived: out of the sidebar, kept
--   actions.promote(chat_id)      a chat made an orchestrator, then opened
--   actions.create(fields)        `createOrchestrator`, then opened: a promise
--                                 of its id

local orchestration = require("@account/orchestration")
local sessions = require("@account/sessions")
local route = require("@app/screens/shell/route")
local locale = require("@ds/locale")
local toast = require("@ds/toast")
local data = require("@ds/orch_data")

local M = {}

-- Which dialog is open, for screens/orchestrator/dialogs (kept across a
-- reload: a dialog open then is there again).
M.new_open = ui.state("orch_new_open", false)
M.settings_open = ui.state("orch_settings_open", false)
M.settings_of = ui.state("orch_settings_of", nil :: string?)
-- The New dialog's project and device to start on (a project's own "+").
M.new_seed = ui.state("orch_new_seed", nil :: { project: string?, device: string? }?)

local function failed(key: string): (any) -> ()
	return function(_err: any)
		toast.show { title = locale.s(key), tone = "danger" }
	end
end

function M.open(id: string)
	route.orchestrator(id)
end

function M.new(seed: { project: string?, device: string? }?)
	ui.batch(function()
		M.new_seed:set(seed)
		M.new_open:set(true)
	end)
end

function M.settings(id: string)
	ui.batch(function()
		M.settings_of:set(id)
		M.settings_open:set(true)
	end)
end

-- One orchestrator's fields patched (`updateOrchestrator`): absent stays.
function M.update(id: string, fields: { [string]: any }): any
	local op: { [string]: any } = { op = "updateOrchestrator", id = id }
	for k, v in fields do
		op[k] = v
	end
	local p = orchestration.op(op)
	p:after(function() end, failed("orch.actions.update_failed"))
	return p
end

function M.set_paused(id: string, on: boolean)
	M.update(id, { paused = on })
end

function M.archive(id: string)
	M.update(id, { archived = true })
	if route.current:peek().name == "orchestrator" and route.current:peek().orchestrator == id then
		route.control()
	end
end

-- A reply's id (`{ id }`), else nil.
local function id_of(reply: any): string?
	if type(reply) == "table" and type(reply.id) == "string" then
		return reply.id
	end
	return if type(reply) == "string" then reply else nil
end

function M.promote(chat_id: string)
	local c = data.chats(sessions.now:peek())[chat_id]
	local name = if c and c.title and c.title ~= "" then c.title else locale.s("orch.actions.untitled")
	orchestration.op({ op = "promoteChat", chatId = chat_id, name = name }):after(function(reply: any)
		route.orchestrator(id_of(reply) or chat_id)
	end, failed("orch.actions.promote_failed"))
end

export type Fields = {
	name: string,
	goal: string?,
	project: string?,
	device: string?,
	harness: string?,
	model: string?,
}

function M.create(f: Fields): any
	local op: { [string]: any } = { op = "createOrchestrator", name = f.name, goal = f.goal or "" }
	op.spaceId = f.project
	op.deviceId = f.device
	op.harness = f.harness
	op.model = f.model
	local p = orchestration.op(op)
	p:after(function(reply: any)
		local id = id_of(reply)
		if id then
			route.orchestrator(id)
		end
	end, failed("orch.actions.create_failed"))
	return p
end

return M

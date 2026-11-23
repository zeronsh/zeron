-- The shell's sidebar on orchestration's sample world
-- (account/orchestration_sample), open (#shell-sidebar) and as its rail
-- (#orch-rail), for tests/sidebar_orch.test.luau. No engine: the
-- `Orchestrate` ops asked are noted in `orch_calls` (`op:key=value,…`, one
-- op a line) and answered at once.

ui.theme.strict("warn")
-- (Orchestrators on, as these are its: off is the default everywhere.)
require("@account/plugins").set({ orchestrators = true })
local ds = require("@ds")
ds.setup()
local sessions = require("@account/sessions")
local orchestration = require("@orchestrators/account/orchestration")
local sample = require("@orchestrators/account/orchestration_sample")
local route = require("@app/screens/shell/route")
route.launched:set(true)
local Sidebar = require("@app/screens/shell/sidebar")

-- The chats as the host has them: crew threads under their orchestrator
-- (`children`), the rest top-level.
local chats: { any }, children: { [string]: { any } } = {}, {}
for _, c: any in sample.chats do
	if c.parent then
		children[c.parent] = children[c.parent] or {}
		table.insert(children[c.parent], c)
	else
		table.insert(chats, c)
	end
end
sessions.now:set({
	state = "ready",
	here = "dev-desk",
	devices = sample.devices,
	projects = {},
	chats = chats,
	archived = {},
	children = children,
	places = { pinned = {}, sections = {} },
	places_ready = true,
} :: any)
orchestration.now:set(sample.now)

local calls = ui.state("orch_calls", "")
local function settled(value: any): any
	return {
		after = function(_self: any, done: (any) -> (), _failed: ((any) -> ())?)
			done(value)
		end,
	}
end
local function said(op: { [string]: any }): string
	local keys: { string } = {}
	for k in op do
		if k ~= "op" then
			table.insert(keys, k)
		end
	end
	table.sort(keys)
	local parts: { string } = {}
	for _, k in keys do
		table.insert(parts, `{k}={tostring(op[k])}`)
	end
	return `{op.op}:{table.concat(parts, ",")}`
end
;(orchestration :: any).op = function(op: { [string]: any }): any
	local line = said(op)
	calls:set(if calls:peek() == "" then line else `{calls:peek()}\n{line}`)
	if op.op == "promoteChat" then
		return settled({ id = op.chatId })
	end
	return settled({ id = "orc-new" })
end

-- Where the route stands, for the tests to read.
local where = ui.state("orch_route", "")
ui.effect(function()
	local c = route.current:get()
	where:set(`{c.name}:{c.orchestrator or c.session or ""}`)
end)

local collapsed = ui.state("orch_sidebar_collapsed", false)
local rail_collapsed = ui.state("orch_rail_collapsed", true)

return ui.row {
	w_full = true,
	h_full = true,
	bg = "$background",
	ui.div { h_full = true, Sidebar.Sidebar { collapsed = collapsed } },
	ui.div { id = "orch-rail", h_full = true, Sidebar.Sidebar { collapsed = rail_collapsed, island = true } },
	ds.toast.Host {},
}

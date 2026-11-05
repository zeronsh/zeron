-- The panel home's host calls (src/session_home.rs), one place, so a
-- fixture stands in for them. Each a promise; a refusal's message is the
-- engine's words, or "unsupported" from an engine before run actions.
--
--   api.actions(space, device?)                 { actions, importable, issue? }
--   api.save_action(space, device?, draft)      the same, after: `draft` is
--                                               { id?, name, command, icon, setup }
--   api.delete_action(space, device?, id)       the same, after
--   api.run_action(space, chat, device?, id)    its terminal's id

local M = {}

-- A host without the call (one from before them, a fixture's) refuses it
-- as an engine from before run actions does: "unsupported".
local function call(name: string, arg: any): any
	local ok, promise = pcall(ui.call, name, arg)
	if ok then
		return promise
	end
	return {
		after = function(_self: any, _done: any, failed: ((any) -> ())?)
			if failed then
				failed({ message = "unsupported" })
			end
		end,
	}
end

function M.actions(space: string, device: string?): any
	return call("home.actions", { space = space, device = device })
end

function M.save_action(space: string, device: string?, draft: { [string]: any }): any
	local arg = table.clone(draft)
	arg.space, arg.device = space, device
	return call("home.save_action", arg)
end

function M.delete_action(space: string, device: string?, id: string): any
	return call("home.delete_action", { space = space, device = device, id = id })
end

function M.run_action(space: string, chat: string, device: string?, id: string): any
	return call("home.run_action", { space = space, chat = chat, device = device, id = id })
end

return M

-- What an orchestrator's side panel remembers, each orchestrator its own,
-- on this device (ui.persist): how wide the panel is, which tab shows,
-- whether the page shows the chat, both, or the panel alone, and which of
-- its sections are folded. And, for the page alone (not kept): what the
-- panel has open in place of its tabs (a card's or a crew thread's detail).
--
--   state.get(id)                       { w?, tab?, mode?, folded? } (tracked)
--   state.set(id, field, value)         one field kept
--   state.folded(id, key, default)      whether section `key` is folded
--   state.fold(id, key, folded)
--   state.detail                        { chat, item? } or nil: the detail
--                                       open in the panel

local M = {}

export type Kept = { w: number?, tab: string?, mode: string?, folded: { [string]: boolean }? }

-- Every orchestrator's, by id.
M.kept = ui.persist("orch_panel", {} :: { [string]: Kept })

-- What the panel shows in place of its tabs: a card's or a crew thread's
-- detail ({ chat = thread id, item = board card id? }).
M.detail = ui.state("orch_panel_detail", nil :: { chat: string?, item: string? }?)

function M.get(id: string?): Kept
	if not id then
		return {}
	end
	return M.kept:get()[id] or {}
end

function M.peek(id: string?): Kept
	if not id then
		return {}
	end
	return M.kept:peek()[id] or {}
end

function M.set(id: string?, field: string, value: any)
	if not id then
		return
	end
	local all = table.clone(M.kept:peek())
	local one = table.clone(all[id] or {}) :: any
	if one[field] == value then
		return
	end
	one[field] = value
	all[id] = one
	M.kept:set(all)
end

function M.folded(id: string?, key: string, default: boolean): boolean
	local f = M.get(id).folded
	local v = f and f[key]
	if v == nil then
		return default
	end
	return v
end

function M.fold(id: string?, key: string, folded: boolean)
	if not id then
		return
	end
	local f = table.clone(M.peek(id).folded or {})
	f[key] = folded
	M.set(id, "folded", f)
end

return M

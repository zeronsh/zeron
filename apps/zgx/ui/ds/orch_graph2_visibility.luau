-- Which chats the new Graph (ds/orch_graph2) lays out: the Map's one rule
-- for every view, ds/orch_map_visibility, when it's there; until then the
-- same rule here, in the same shape, so the Graph needn't change when it
-- lands.
--
--   vis.context(world)            Ctx, from orchestration's state and the
--                                 setting (read, so a change lays out again)
--   vis.visible(chat, now, ctx)   "show" | "fade" | "hide", opacity
--   vis.now()                     the time the Map judges at (the shared
--                                 clock, on each minute; else os.time())
--   vis.next_change(chat, now, ctx)
--                                 seconds until the answer changes on its
--                                 own, for a timer of the Graph's: nil with
--                                 the shared clock, which moves itself
--   vis.mode, vis.MODES           the setting: "active" | "recent" | "all"
--   vis.own_setting               true while it's this adapter's own (the
--                                 Map's menu offers it then)
--
-- Always shown: an orchestrator's own; at work, waiting, asking; finished
-- or failed and unread; an open ask not put off; a card in Review.
-- Otherwise "all" shows it, "active" hides it, "recent" shows it for WINDOW
-- after it last moved, fading through the window's last half (to FADE_MIN),
-- then hides it.

local M = {}

local shared: any = nil
do
	local ok, mod = pcall(require, "@ds/orch_map_visibility")
	if ok and type(mod) == "table" and type(mod.visible) == "function" and type(mod.context) == "function" then
		shared = mod
	end
end

M.MODES = { "active", "recent", "all" }
M.DEFAULT = "recent"
M.WINDOW = if shared and shared.WINDOW then shared.WINDOW else 60 * 60
M.FADE_FROM = if shared and shared.FADE_FROM then shared.FADE_FROM else 0.5
M.FADE_MIN = if shared and shared.FADE_MIN then shared.FADE_MIN else 0.35
M.own_setting = shared == nil
-- (Without an app, a pure rule's tests: no state, the default.)
local has_ui = type(ui) == "table" or type(ui) == "userdata"
M.mode = if shared and shared.mode
	then shared.mode
	elseif has_ui then ui.persist("control_map_show", M.DEFAULT)
	else ({ get = function(): string
		return M.DEFAULT
	end, peek = function(): string
		return M.DEFAULT
	end, set = function(_v: string) end } :: any)

export type Ctx = {
	mode: string,
	window: number,
	pinned: { [string]: boolean },
	asking: { [string]: boolean },
	review: { [string]: boolean },
}

local LIVE: { [string]: boolean } = { working = true, connecting = true, waiting = true, input = true, approval = true }
local UNREAD: { [string]: boolean } = { done = true, failed = true }

local function valid(mode: any): string
	return if table.find(M.MODES, mode) then mode else M.DEFAULT
end

function M.now(): number
	if shared and shared.now then
		return (shared :: any).now()
	end
	return os.time()
end

function M.context(world: any): Ctx
	local mode = valid(M.mode:get())
	local now = world.now or {}
	if shared then
		return (shared :: any).context(now, mode)
	end
	local ctx: Ctx = { mode = mode, window = M.WINDOW, pinned = {}, asking = {}, review = {} }
	for _, o in now.orchestrators or {} do
		ctx.pinned[o.id] = true
	end
	local t = os.time()
	for _, a in now.asks or {} do
		local put_off = type(a.deferred_until) == "number" and a.deferred_until > t
		if a.state == "open" and a.from_chat and not put_off then
			ctx.asking[a.from_chat] = true
		end
	end
	for _, it in now.items or {} do
		if it.state == "review" and not it.removed then
			for _, id in it.threads or {} do
				ctx.review[id] = true
			end
		end
	end
	return ctx
end

function M.visible(chat: any, now: number, ctx: Ctx): (string, number)
	if shared then
		return shared.visible(chat, now, ctx)
	end
	if chat == nil then
		return "show", 1
	end
	if chat.archived then
		return "hide", 0
	end
	local id = chat.id
	if (id and (ctx.pinned[id] or ctx.asking[id] or ctx.review[id])) or LIVE[chat.status or ""] or UNREAD[chat.status or ""] then
		return "show", 1
	end
	if ctx.mode == "all" then
		return "show", 1
	elseif ctx.mode == "active" then
		return "hide", 0
	end
	local age = math.max(0, now - (chat.updated or chat.created or 0))
	local window = math.max(1, ctx.window)
	if age >= window then
		return "hide", 0
	end
	local from = window * M.FADE_FROM
	if age < from then
		return "show", 1
	end
	return "fade", 1 - (age - from) / (window - from) * (1 - M.FADE_MIN)
end

-- (The fade's steps are tenths: the next tenth, or the hide.)
function M.next_change(chat: any, now: number, ctx: Ctx): number?
	if shared and shared.clock then
		return nil
	end
	if chat == nil or ctx.mode ~= "recent" then
		return nil
	end
	local v = M.visible(chat, now, ctx)
	if v == "hide" then
		return nil
	end
	local at = chat.updated or chat.created or 0
	local window = math.max(1, ctx.window)
	local from = window * M.FADE_FROM
	local age = math.max(0, now - at)
	if age < from then
		return math.max(1, from - age)
	end
	local step = (window - from) / 10
	return math.max(1, step - ((age - from) % step))
end

return M

-- Which chats the Map shows: one rule for every view (Graph, Topology,
-- Timeline, Machines, and the Control room's Machines widget). Separate from
-- archiving. Pure: computed from live state each time, so nothing caches a
-- hide and any new activity (a message, a turn, an ask) brings a chat back
-- at once.
--
--   Visibility.visible(chat, now, ctx) -> "show" | "fade" | "hide", opacity
--   Visibility.context(orch_now, mode?, window?) -> Ctx
--   Visibility.filter(list, now, ctx, id_of?) -> { { item, verdict, opacity } }
--   Visibility.mode            the person's choice (ui.persist, this device):
--                              "active" | "recent" (default) | "all"
--   Visibility.MODES, Visibility.DEFAULT, Visibility.WINDOW (seconds)
--   Visibility.clock           the time the Map judges at, moved on each
--                              minute while something reads it (a fading
--                              chat fades, then hides, with nothing else
--                              moving)
--   Visibility.key(chats, now, ctx)  each chat's verdict, as a string: for a
--                              view's own layout cache, so a cached layout
--                              never keeps a hide (or a show)
--
-- A chat always shows when it's an orchestrator's or the Brain dump's own
-- (not archived);
-- working, waiting on its own work, or asking (input, approval); finished or
-- failed and not seen since (unread); with an open ask of its, not put off;
-- or on a card in Review. Otherwise: "all" shows it; "active" hides it;
-- "recent" shows it for the window after it last moved (`updated`, else
-- `created`), fading through the window's last part (opacity from 1 down to
-- FADE_MIN), then hides it. A chat the Map doesn't know yet (nil) shows.

local M = {}

export type Mode = "active" | "recent" | "all"
export type Verdict = "show" | "fade" | "hide"
export type Ctx = {
	mode: Mode,
	-- Seconds a finished chat stays ("recent").
	window: number,
	-- Chats that always show, by id: orchestrators' and the Brain dump's own.
	pinned: { [string]: boolean },
	-- Chats with an open ask not put off, by id.
	asking: { [string]: boolean },
	-- Chats on a card in Review, by id.
	review: { [string]: boolean },
}

M.MODES = { "active", "recent", "all" } :: { Mode }
M.DEFAULT = "recent" :: Mode
-- How long a finished chat stays in "recent" (seconds).
M.WINDOW = 60 * 60
-- Where in the window it starts to fade (its share), and how faint it gets.
M.FADE_FROM = 0.5
M.FADE_MIN = 0.35

-- The person's choice (Control room view settings, this device's).
-- (Without an app, a pure rule's tests: no state, the default.)
local has_ui = type(ui) == "table" or type(ui) == "userdata"
M.mode = if has_ui then ui.persist("control_map_show", M.DEFAULT) else ({ get = function() return M.DEFAULT end, peek = function() return M.DEFAULT end } :: any)

-- The Map's clock: Unix seconds, on to the next minute each minute.
M.clock = if has_ui then ui.shared("orch_map_visibility.clock", os.time()) else ({ get = function() return os.time() end, peek = function() return os.time() end } :: any)
local ticking = false
local function tick()
	M.clock:set(os.time())
	ui.after(60 * 1000, tick)
end
-- Read it through this, so it starts moving the first time it's read.
function M.now(): number
	if not ticking then
		ticking = true
		ui.after(60 * 1000, tick)
	end
	return M.clock:get()
end

-- Under way or asking for the person: always shown.
local LIVE: { [string]: boolean } = { working = true, connecting = true, waiting = true, input = true, approval = true }
-- Finished or failed and not looked at since: unread.
local UNREAD: { [string]: boolean } = { done = true, failed = true }

local function valid(mode: any): Mode
	if mode == "active" or mode == "recent" or mode == "all" then
		return mode
	end
	return M.DEFAULT
end

-- What `visible` needs beyond the chat, from orchestration's state (its
-- orchestrators, asks and items) and the mode (else the person's).
function M.context(orch_now: any, mode: any?, window: number?): Ctx
	local now = orch_now or {}
	local ctx: Ctx = {
		mode = valid(if mode ~= nil then mode else M.mode:peek()),
		window = window or M.WINDOW,
		pinned = {},
		asking = {},
		review = {},
	}
	for _, o in now.orchestrators or {} do
		if not o.archived then
			ctx.pinned[o.id] = true
		end
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

-- Whether `chat` (a sessions row: { id, status?, updated?, created?,
-- archived? }) shows at `now` (Unix seconds), and how strongly.
function M.visible(chat: any, now: number, ctx: Ctx): (Verdict, number)
	if chat == nil then
		return "show", 1
	end
	if chat.archived then
		return "hide", 0
	end
	local id = chat.id
	local status = chat.status or ""
	if (id and (ctx.pinned[id] or ctx.asking[id] or ctx.review[id])) or LIVE[status] or UNREAD[status] then
		return "show", 1
	end
	if ctx.mode == "all" then
		return "show", 1
	end
	if ctx.mode == "active" then
		return "hide", 0
	end
	local at = chat.updated or chat.created or 0
	local age = math.max(0, now - at)
	local window = math.max(1, ctx.window)
	if age >= window then
		return "hide", 0
	end
	local from = window * M.FADE_FROM
	if age < from then
		return "show", 1
	end
	local t = (age - from) / (window - from)
	return "fade", 1 - t * (1 - M.FADE_MIN)
end

-- Each chat's verdict (its opacity in tenths while it fades), as one
-- string.
function M.key(chats: { [string]: any }, now: number, ctx: Ctx): string
	local parts = { ctx.mode }
	for id, c in chats do
		local v, a = M.visible(c, now, ctx)
		if v ~= "show" then
			table.insert(parts, `{id}:{if v == "hide" then "h" else math.floor(a * 10 + 0.5)}`)
		end
	end
	table.sort(parts)
	return table.concat(parts, ",")
end

-- `list` with each item's verdict (`id_of(item)` → the chat, by default the
-- item itself), the hidden ones left out.
function M.filter(list: { any }, now: number, ctx: Ctx, chat_of: ((any) -> any)?): { { item: any, verdict: Verdict, opacity: number } }
	local out = {}
	for _, item in list do
		local v, a = M.visible(if chat_of then chat_of(item) else item, now, ctx)
		if v ~= "hide" then
			table.insert(out, { item = item, verdict = v, opacity = a })
		end
	end
	return out
end

return M

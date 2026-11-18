-- What a thread is called wherever orchestration draws it: the Map's views
-- (Machines, Graph, Timeline, Topology), the Control room's Machines, an
-- orchestrator's crew and the sidebar's crew rows. One rule, so a thread
-- is named alike everywhere and never drawn as a bare dot:
--
--   its own title (a side chat's or crew thread's `name`: the title it was
--   given, not what it last said), else, for a crew thread, its role in the
--   crew, else its board item's title, else "Crew thread" and a short id.
--   An orchestrator: its name.
--
--   name.thread(chat, member, item, id)  the rule over what's at hand
--   name.of(now, chats, id)              the same, looked up
--   name.index(now)                      crew and items by id, for many
--   name.from(index, chats, id)          the same with an index
--   name.own(chat)                       a chat's own title, or nil
--   name.fallback(member, item)          a crew thread's name without its
--                                        own title (role, item), or nil
--   name.crew(chat, fallback, id)        a crew thread's, fallback given
--   name.unnamed(id)                     the last resort

local locale = require("@ds/locale")

local M = {}

local function text(s: any): string?
	if type(s) ~= "string" then
		return nil
	end
	local t = string.match(s, "^%s*(.-)%s*$")
	return if t and t ~= "" then t else nil
end

-- A chat's own title. A side chat's or crew thread's row (under its
-- parent) says it as `name` (its `title` falls back to what it last said);
-- a top-level chat's `title` is its own.
function M.own(chat: any?): string?
	if type(chat) ~= "table" then
		return nil
	end
	if chat.parent ~= nil then
		return text(chat.name)
	end
	return text(chat.name) or text(chat.title)
end

-- A crew thread's name without a title of its own: its role in the crew,
-- else its board item's title.
function M.fallback(member: any?, item: any?): string?
	return (member and text(member.role)) or (item and text(item.title)) or nil
end

-- The last resort: "Crew thread" and the first characters of its id.
function M.unnamed(id: string?): string
	local short = string.sub(id or "", 1, 6)
	local said = text(locale.s("ds.orch.crew.unnamed", { id = short }))
	-- (Words not loaded, as in a bare test: the key, and the id after it.)
	if not said or string.find(said, short, 1, true) == nil then
		return if said then `{said} {short}` else short
	end
	return said
end

-- A crew thread's name, its fallback (`fallback(member, item)`) worked
-- out already: its own title, else that, else the last resort.
function M.crew(chat: any?, fallback: string?, id: string?): string
	return M.own(chat) or text(fallback) or M.unnamed(id)
end

-- The rule, over a chat's row (or none: not heard yet), its crew row (or
-- none: not crew) and its item (or none).
function M.thread(chat: any?, member: any?, item: any?, id: string?): string
	if member ~= nil then
		return M.crew(chat, M.fallback(member, item), id)
	end
	-- (Not crew: a side chat untitled is called what its host calls it.)
	return M.own(chat) or (chat and text(chat.title)) or M.unnamed(id)
end

export type Index = {
	orchestrators: { [string]: any },
	crew: { [string]: any },
	items: { [string]: any },
}

function M.index(now: any): Index
	local out: Index = { orchestrators = {}, crew = {}, items = {} }
	for _, o in (now and now.orchestrators) or {} do
		out.orchestrators[o.id] = o
	end
	for _, m in (now and now.crew) or {} do
		if not m.released and m.chat_id then
			out.crew[m.chat_id] = m
		end
	end
	for _, it in (now and now.items) or {} do
		out.items[it.id] = it
	end
	return out
end

function M.from(index: Index, chats: { [string]: any }?, id: string): string
	local o = index.orchestrators[id]
	local chat = chats and chats[id]
	if o then
		return text(o.name) or M.own(chat) or M.unnamed(id)
	end
	local m = index.crew[id]
	local item = m and m.item_id and index.items[m.item_id]
	return M.thread(chat, m, item, id)
end

function M.of(now: any, chats: { [string]: any }?, id: string): string
	return M.from(M.index(now), chats, id)
end

return M

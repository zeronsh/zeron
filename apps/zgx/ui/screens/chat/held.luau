-- Messages kept back from a chat whose Cloud machine can't take them yet (its
-- size too small, its files gone): each the send it was, its words, and when.
-- Kept across launches, so nothing written is lost; shown in the chat's queue,
-- removable there; sent once the machine can take them
-- (screens/chat/composer). Plain data, by chat.
--
--   Held.add(chat, { id, at, text, req })   Held.of(chat)   Held.remove(chat, id)

local M = {}

export type Entry = { id: string, at: number, text: string, req: { [string]: any } }

M.all = ui.persist("cloud_held_sends", {} :: { [string]: { Entry } })

-- A held message's row in the queue, by this prefix: told from the queue's own.
M.PREFIX = "held:"

function M.of(chat: string?): { Entry }
	if not chat then
		return {}
	end
	return M.all:get()[chat] or {}
end

function M.add(chat: string, entry: Entry)
	local next_all = table.clone(M.all:peek())
	local list = table.clone(next_all[chat] or {})
	table.insert(list, entry)
	next_all[chat] = list
	M.all:set(next_all)
end

function M.remove(chat: string, id: string): Entry?
	local list = M.all:peek()[chat]
	if not list then
		return nil
	end
	local kept, taken = {}, nil
	for _, e in list do
		if e.id == id then
			taken = e
		else
			table.insert(kept, e)
		end
	end
	local next_all = table.clone(M.all:peek())
	next_all[chat] = if #kept > 0 then kept else nil
	M.all:set(next_all)
	return taken
end

-- The held message a queue row stands for (its id, as the queue knows it).
function M.id_of(row: string): string?
	if string.sub(row, 1, #M.PREFIX) == M.PREFIX then
		return string.sub(row, #M.PREFIX + 1)
	end
	return nil
end

return M

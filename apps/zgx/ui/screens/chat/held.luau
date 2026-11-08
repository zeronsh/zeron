-- Messages kept back from a chat whose Cloud machine can't take them yet (its
-- size too small, its files gone): each the send it was, its words, and when.
-- Kept across launches, so nothing written is lost; shown in the chat's queue,
-- removable there; sent once the machine can take them
-- (screens/chat/composer). Plain data, by chat. One whose send was refused
-- is back in its place, `refused`: it goes again only when the person sends it.
--
--   Held.add(chat, { id, at, text, req, refused? })   Held.of(chat)   Held.remove(chat, id)

local M = {}

export type Entry = { id: string, at: number, text: string, req: { [string]: any }, refused: boolean? }

M.all = ui.persist("cloud_held_sends", {} :: { [string]: { Entry } })

-- A held message's row in the queue, by this prefix: told from the queue's own.
M.PREFIX = "held:"

function M.of(chat: string?): { Entry }
	if not chat then
		return {}
	end
	return M.all:get()[chat] or {}
end

-- In the order they were written (one put back goes back in its place).
function M.add(chat: string, entry: Entry)
	local next_all = table.clone(M.all:peek())
	local list = table.clone(next_all[chat] or {})
	local at = #list + 1
	for i, e in list do
		if e.at > entry.at then
			at = i
			break
		end
	end
	table.insert(list, at, entry)
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

-- The queue as the composer shows it: the engine's rows (the session's
-- `queue`, src/chat.rs), and what was just asked of it, shown the frame it
-- was asked rather than once the engine answers (a round trip, and for a
-- chat hosted on another device, the network's: a third of a second and
-- more, and a queued message's files upload first).
--
--   queued.rows(session, engine)   the rows to show
--   queued.add(session, engine, text, cargo?)   a message queued: its row
--                                  at once (an id of its own, `sending`)
--   queued.added(session, row, id) the engine took it, as `id`
--   queued.hide(session, ids) / queued.unhide(session, ids)
--                                  rows steered, sent or removed: gone at once,
--                                  back if the engine refuses
--   queued.front(session, id) / queued.unfront(session, id)
--                                  a row sent next: first at once
--   queued.forget(session, row)    a row queued that didn't go
--   queued.prune(session, engine)  what the engine has caught up with, let go
--
-- A row added keeps its place, and its key (`key`), as the engine's copy
-- replaces it: matched by the engine's id once it answers, or before that
-- by its words, a row the engine didn't have when it was added.

local M = {}

type Add = {
	id: string,
	text: string,
	cargo: { any }?,
	-- The engine's rows when it was added: a new one with its words is it.
	before: { [string]: boolean },
	-- The engine's id for it, once it answers, and when it answered.
	real: string?,
	answered: number?,
}
type Overlay = {
	adds: { Add },
	gone: { [string]: boolean },
	front: string?,
	-- An engine row's key: the row added that became it.
	keys: { [string]: string },
}

-- Each session's, by id. Shared: a reload keeps what's under way.
local OVERLAY = ui.shared("chat.queue.overlay", {} :: { [string]: Overlay })
local made = ui.shared("chat.queue.made", 0)

-- How long a row the engine took may wait to be listed before it's let go:
-- one sent on at once (the turn had ended) is never listed.
local LISTED_MS = 1500

local function of(all: { [string]: Overlay }, session: string): Overlay
	return all[session] or { adds = {}, gone = {}, keys = {} }
end

-- `fn` given a copy of the session's overlay to change, then kept.
local function change(session: string, fn: (Overlay) -> ())
	local all = table.clone(OVERLAY:peek())
	local was = of(all, session)
	local o: Overlay = {
		adds = table.clone(was.adds),
		gone = table.clone(was.gone),
		front = was.front,
		keys = table.clone(was.keys),
	}
	fn(o)
	all[session] = o
	OVERLAY:set(all)
end

-- Which engine row each add is now (nil: not yet), by the add's index.
local function matches(o: Overlay, engine: { any }): { [number]: string }
	local claimed: { [string]: boolean } = {}
	local out: { [number]: string } = {}
	for i, a in o.adds do
		for _, r in engine do
			if a.real and r.id == a.real then
				out[i] = r.id
				break
			end
		end
	end
	for _, id in out do
		claimed[id] = true
	end
	for i, a in o.adds do
		if not out[i] and not a.real then
			for _, r in engine do
				if not claimed[r.id] and not a.before[r.id] and r.text == a.text then
					out[i] = r.id
					claimed[r.id] = true
					break
				end
			end
		end
	end
	return out
end

function M.rows(session: string?, engine: { any }): { any }
	if not session then
		return engine
	end
	local o = OVERLAY:get()[session]
	if not o then
		return engine
	end
	local matched = matches(o, engine)
	local keys = table.clone(o.keys)
	for i, id in matched do
		keys[id] = o.adds[i].id
	end
	-- (A row hidden while on its way stays hidden as the engine's.)
	local hidden = table.clone(o.gone)
	for i, id in matched do
		if o.gone[o.adds[i].id] then
			hidden[id] = true
		end
	end
	local out: { any } = {}
	for _, r in engine do
		if not hidden[r.id] then
			local key = keys[r.id]
			if key then
				local row = table.clone(r)
				row.key = key
				table.insert(out, row)
			else
				table.insert(out, r)
			end
		end
	end
	if o.front then
		for i, r in out do
			if r.id == o.front and i > 1 then
				table.remove(out, i)
				table.insert(out, 1, r)
				break
			end
		end
	end
	local now = ui.now()
	for i, a in o.adds do
		local let_go = a.answered ~= nil and now - a.answered > LISTED_MS
		if not matched[i] and not let_go and not o.gone[a.id] then
			table.insert(out, { id = a.id, key = a.id, text = a.text, cargo = a.cargo, sending = true })
		end
	end
	return out
end

function M.add(session: string, engine: { any }, text: string, cargo: { any }?): string
	local n = made:peek() + 1
	made:set(n)
	local id = `queued:{n}`
	local before: { [string]: boolean } = {}
	for _, r in engine do
		before[r.id] = true
	end
	change(session, function(o)
		table.insert(o.adds, { id = id, text = text, cargo = cargo, before = before })
	end)
	return id
end

function M.added(session: string, row: string, id: string?)
	change(session, function(o)
		for i, a in o.adds do
			if a.id == row then
				local now = table.clone(a)
				now.real = id
				now.answered = ui.now()
				o.adds[i] = now
			end
		end
	end)
	-- Taken but never listed (sent on at once): let go once it's had time.
	ui.after(LISTED_MS + 100, function()
		local o = OVERLAY:peek()[session]
		if not o then
			return
		end
		for _, a in o.adds do
			if a.id == row then
				M.forget(session, row)
			end
		end
	end)
end

function M.forget(session: string, row: string)
	change(session, function(o)
		o.gone[row] = nil
		for i, a in o.adds do
			if a.id == row then
				table.remove(o.adds, i)
				break
			end
		end
	end)
end

function M.hide(session: string, ids: { string })
	change(session, function(o)
		for _, id in ids do
			o.gone[id] = true
		end
	end)
end

function M.unhide(session: string, ids: { string })
	change(session, function(o)
		for _, id in ids do
			o.gone[id] = nil
		end
	end)
end

function M.front(session: string, id: string)
	change(session, function(o)
		o.front = id
	end)
end

function M.unfront(session: string, id: string)
	change(session, function(o)
		if o.front == id then
			o.front = nil
		end
	end)
end

-- What the engine has caught up with, let go: rows added that it lists
-- now (their keys kept while it does), rows hidden it no longer lists, a
-- row sent next that it lists first or not at all.
function M.prune(session: string?, engine: { any })
	if not session then
		return
	end
	local o = OVERLAY:peek()[session]
	if not o then
		return
	end
	local listed: { [string]: boolean } = {}
	for _, r in engine do
		listed[r.id] = true
	end
	-- (A row added and hidden waits for the engine's copy.)
	local adding: { [string]: boolean } = {}
	for _, a in o.adds do
		adding[a.id] = true
	end
	local matched = matches(o, engine)
	local stale = false
	for i in matched do
		stale = stale or o.adds[i] ~= nil
	end
	for id in o.gone do
		stale = stale or not (listed[id] or adding[id])
	end
	for id in o.keys do
		stale = stale or not listed[id]
	end
	if o.front and (not listed[o.front] or (engine[1] and engine[1].id == o.front)) then
		stale = true
	end
	if not stale then
		return
	end
	change(session, function(n)
		local adds: { Add } = {}
		for i, a in n.adds do
			if matched[i] then
				n.keys[matched[i]] = a.id
				if n.gone[a.id] then
					n.gone[a.id] = nil
					n.gone[matched[i]] = true
				end
			else
				table.insert(adds, a)
			end
		end
		n.adds = adds
		local still: { [string]: boolean } = {}
		for _, a in n.adds do
			still[a.id] = true
		end
		for id in table.clone(n.gone) do
			if not (listed[id] or still[id]) then
				n.gone[id] = nil
			end
		end
		for id in table.clone(n.keys) do
			if not listed[id] then
				n.keys[id] = nil
			end
		end
		if n.front and (not listed[n.front] or (engine[1] and engine[1].id == n.front)) then
			n.front = nil
		end
	end)
end

return M

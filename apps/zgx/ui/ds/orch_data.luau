-- Orchestration's data joined for drawing: plain functions over
-- `orchestration.now` (docs/design/orchestrators.md, "Bridge") and
-- `sessions.now`, shared by the Control room, an orchestrator's page and
-- the sidebar. Nothing here reads a state: callers pass what they read, so
-- a computed of theirs follows exactly what it used.
--
--   data.chats(sessions_now)          every chat by id (top-level, side
--                                     chats and crew threads, archived)
--   data.orchestrators(now)           by id
--   data.crew_by_chat(now)            the live crew rows, by thread id
--   data.hue_of(now, chats, chat_id)  the hue a chat wears: its own as an
--                                     orchestrator, its orchestrator's as
--                                     crew, else nil
--   data.target(now, chats, target)   a route's destination:
--                                     { label, hue?, kind, id? }
--   data.segments(text, routes)       a dump's text cut at its routes'
--                                     spans: { { text, route? } }, in order
--   data.needs(now, sessions_now)     what needs the person: open asks and
--                                     chats awaiting input (one row a chat:
--                                     its ask, `input` set, when it has
--                                     one), urgent first, then the longest
--                                     waiting
--   data.ACTIVITY                     each activity's glyph and word key
--   data.day_start(t)                 local midnight of `t` (Unix seconds)
--   data.shown_today(item, now)       whether an item stands on the board
--                                     today: any not done; a done one,
--                                     done since local midnight (the
--                                     board's Done column and its count)

local M = {}

export type Chat = { [string]: any }

function M.chats(sessions_now: any): { [string]: Chat }
	local out: { [string]: Chat } = {}
	if not sessions_now then
		return out
	end
	for _, c in sessions_now.archived or {} do
		out[c.id] = c
	end
	for _, kids in sessions_now.children or {} do
		for _, c in kids do
			out[c.id] = c
		end
	end
	for _, c in sessions_now.chats or {} do
		out[c.id] = c
	end
	return out
end

function M.orchestrators(now: any): { [string]: any }
	local out = {}
	for _, o in (now and now.orchestrators) or {} do
		out[o.id] = o
	end
	return out
end

function M.crew_by_chat(now: any): { [string]: any }
	local out = {}
	for _, m in (now and now.crew) or {} do
		if not m.released then
			out[m.chat_id] = m
		end
	end
	return out
end

function M.items(now: any): { [string]: any }
	local out = {}
	for _, it in (now and now.items) or {} do
		out[it.id] = it
	end
	return out
end

function M.hue_of(now: any, chats: { [string]: Chat }?, chat_id: string?): number?
	if not chat_id then
		return nil
	end
	for _, o in (now and now.orchestrators) or {} do
		if o.id == chat_id then
			return o.hue
		end
	end
	for _, m in (now and now.crew) or {} do
		if m.chat_id == chat_id then
			for _, o in now.orchestrators or {} do
				if o.id == m.orchestrator_id then
					return o.hue
				end
			end
		end
	end
	-- A chat under an orchestrator's chat, not (yet) in its crew.
	local c = chats and chats[chat_id]
	if c and c.parent then
		for _, o in (now and now.orchestrators) or {} do
			if o.id == c.parent then
				return o.hue
			end
		end
	end
	return nil
end

-- A chat's title, else an orchestrator's name, else nil.
function M.name_of(now: any, chats: { [string]: Chat }?, id: string?): string?
	if not id then
		return nil
	end
	for _, o in (now and now.orchestrators) or {} do
		if o.id == id then
			return o.name
		end
	end
	local c = chats and chats[id]
	return if c then c.title else nil
end

export type Target = { label: string, hue: number?, kind: string, id: string? }

-- Where a route goes, as a chip names it. `words` gives the plain words for
-- the places that have no name of their own (`words(kind)`: "inbox",
-- "new_thread", "new_orchestrator", "note", "unsure").
function M.target(now: any, chats: { [string]: Chat }?, target: any, words: (string) -> string): Target
	local kind = (target and target.kind) or "unsure"
	local id = target and target.id
	if kind == "orchestrator" or kind == "new_orchestrator" then
		for _, o in (now and now.orchestrators) or {} do
			if o.id == id then
				return { label = o.name, hue = o.hue, kind = kind, id = id }
			end
		end
		return { label = words(kind), kind = kind, id = id }
	elseif kind == "chat" or kind == "new_thread" then
		local name = M.name_of(now, chats, id)
		return { label = name or words(kind), hue = M.hue_of(now, chats, id), kind = kind, id = id }
	elseif kind == "item" then
		local it = M.items(now)[id or ""]
		local owner = it and it.orchestrator_id
		local hue = nil
		if owner then
			hue = M.hue_of(now, chats, owner)
		end
		return { label = if owner then (M.name_of(now, chats, owner) or words("inbox")) else words("inbox"), hue = hue, kind = kind, id = id }
	end
	return { label = words(kind), kind = kind, id = id }
end

export type Segment = { text: string, route: number? }

-- `text` cut at its routes' spans (0-based byte offsets, end exclusive),
-- each piece naming the route it belongs to (its index in `routes`). A
-- span that overlaps an earlier one, or runs past the text, is left out;
-- a route with no span is found by its excerpt where it can be.
function M.segments(text: string, routes: { any }): { Segment }
	local spans: { { from: number, to: number, route: number } } = {}
	for i, r in routes or {} do
		local from, to
		if r.span and type(r.span.from) == "number" and type(r.span.to) == "number" then
			from, to = r.span.from, r.span.to
		elseif type(r.excerpt) == "string" and r.excerpt ~= "" then
			local s, e = string.find(text, r.excerpt, 1, true)
			if s then
				from, to = s - 1, e
			end
		end
		if from and to and from >= 0 and to <= #text and to > from then
			table.insert(spans, { from = from, to = to, route = i })
		end
	end
	table.sort(spans, function(a: any, b: any): boolean
		return a.from < b.from
	end)
	local out: { Segment } = {}
	local at = 0
	for _, s in spans do
		if s.from >= at then
			if s.from > at then
				table.insert(out, { text = string.sub(text, at + 1, s.from) })
			end
			table.insert(out, { text = string.sub(text, s.from + 1, s.to), route = s.route })
			at = s.to
		end
	end
	if at < #text then
		table.insert(out, { text = string.sub(text, at + 1) })
	end
	return out
end

export type Need = {
	id: string,
	kind: string, -- "ask" | "input"
	urgent: boolean,
	at: number,
	ask: any?,
	-- An ask whose chat awaits input too: the row opens the chat as well.
	input: boolean?,
	chat: Chat?,
	from: string?,
	hue: number?,
}

-- What needs the person, newest asks and waiting chats together: urgent
-- first, then the one waiting longest.
function M.needs(now: any, sessions_now: any): { Need }
	local chats = M.chats(sessions_now)
	local out: { Need } = {}
	-- A chat awaiting input with an ask open too is one row: the ask's, with
	-- a way into the chat (`input`).
	local waiting: { [string]: boolean } = {}
	for id, c in chats do
		if (c.status == "input" or c.status == "approval") and not c.archived then
			waiting[id] = true
		end
	end
	local asking: { [string]: boolean } = {}
	for _, a in (now and now.asks) or {} do
		if a.state == "open" then
			asking[a.from_chat] = true
			table.insert(out, {
				id = `ask:{a.id}`,
				kind = "ask",
				urgent = a.urgent == true,
				at = a.created or 0,
				ask = a,
				input = waiting[a.from_chat] == true,
				chat = chats[a.from_chat],
				from = a.from_chat,
				hue = M.hue_of(now, chats, a.from_chat) or (if a.orchestrator_id then M.hue_of(now, chats, a.orchestrator_id) else nil),
			})
		end
	end
	for id, c in chats do
		if waiting[id] and not asking[id] then
			table.insert(out, {
				id = `chat:{id}`,
				kind = "input",
				urgent = false,
				at = c.updated or 0,
				chat = c,
				from = id,
				hue = M.hue_of(now, chats, id),
			})
		end
	end
	table.sort(out, function(a: Need, b: Need): boolean
		if a.urgent ~= b.urgent then
			return a.urgent
		end
		if a.at ~= b.at then
			return a.at < b.at
		end
		return a.id < b.id
	end)
	return out
end

-- Each activity's glyph (ds/icon); its word is `ds.orch.activity.<kind>`.
M.ACTIVITY = {
	thinking = "lightbulb-minimalistic",
	reading = "eye",
	editing = "pen",
	running = "terminal",
	testing = "action-test",
	building = "action-build",
	installing = "download",
	merging = "git-merge",
	pushing = "upload-minimalistic",
	reviewing = "search",
	browsing = "global",
	delegating = "users-group-rounded",
}

-- A route's answer as one line: the engine's own (`answer`, condensed,
-- with `answer_full` beside it), or, from an engine that sends the reply
-- whole, its first sentence, its blocks' marks (headings, lists, quotes)
-- left out; at most 160 characters.
function M.answer_line(route: any): string
	local a = route and route.answer
	if type(a) ~= "string" or a == "" then
		return ""
	end
	if type(route.answer_full) == "string" and route.answer_full ~= "" then
		return a
	end
	local s = string.gsub(a, "\n%s*[#>*%-]+%s*", " ")
	s = string.gsub(s, "^%s*[#>*%-]+%s*", "")
	s = string.gsub(s, "%s+", " ")
	-- (A sentence ends at its stop, and any marks closing round it.)
	local first = string.match(s, "^(.-[%.!?][%*_`]*)%s") or s
	if #first > 160 then
		first = string.sub(first, 1, 157) .. "…"
	end
	return first
end

-- Local midnight of `t`.
function M.day_start(t: number): number
	local d = os.date("*t", t) :: any
	return t - (d.hour * 3600 + d.min * 60 + d.sec)
end

-- Whether an item stands on the board today: one not done always; a done
-- one when it was done (else last changed) since local midnight. `now`
-- in Unix seconds (os.time()'s when left out).
function M.shown_today(item: any, now: number?): boolean
	if item.state ~= "done" then
		return true
	end
	local at = item.done or item.updated or 0
	return at >= M.day_start(now or os.time())
end

-- A todo's fraction, or nil.
function M.todo_fraction(todo: any): number?
	if type(todo) ~= "table" or type(todo.total) ~= "number" or todo.total <= 0 then
		return nil
	end
	return math.clamp((todo.done or 0) / todo.total, 0, 1)
end

return M

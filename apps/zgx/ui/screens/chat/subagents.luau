--!strict
-- A chat's subagents as each reply's subagent block lists them (ds/subagents
-- in ds/transcript), from every harness:
--
-- - its spawns, which the reply carries (src/chat.rs `subagents`: Claude
--   Code's `Task`/`Agent`, a Codex spawn, any harness's `Agent` call), by
--   their call's id: what stops one (`StopToolCall`), and the doc it runs
--   in (`ref`), what opens its lane;
-- - the chats its agent made as subagents through the Zeron MCP server
--   (`create_chat` with `kind: "subagent"`, or none with a parent: the
--   row's `childKind`; src/sessions.rs `subagent_chats`), each in the reply
--   that was being written when it was made. Stopping one interrupts it;
--   opening it opens its lane.
--
-- Each joined with what's known of it: its agent's mark, its model, what
-- it's on (a chat's latest words) or what it came back with, and how long
-- it took (a chat's own times; a spawn's, from when this window first saw
-- it run to when it stopped).
--
--   local subs = Subagents.use(function() return session_id end)
--   subs.items(message)       its block's items (tracked)
--   subs.stop(sub, settle)    its row's Stop
--   subs.open(sub)            its row's Open: its lane in the panel

local locale = require("@ds/locale")
local sessions = require("@account/sessions")
local store = require("@app/screens/chat/store")
local lanes = require("@app/screens/chat/lanes")
local shared = require("@app/screens/chat/shared")

local M = {}

-- How far a chat's creation may run ahead of its reply's start (the two
-- clocks are its host's, but a reply's entry is stamped as it begins).
local SLACK = 5
-- A chat made a moment ago that hasn't said anything yet: starting, not
-- idle.
local STARTING = 120

-- When this window saw each spawn start and stop (`os.time()` seconds), by
-- its call's id: kept across a reload.
local TIMES = ui.state("chat_subagent_times", {} :: { [string]: { start: number?, stop: number? } })

-- A mark for a harness: ds.Mark's name, or a registry agent's harness.
local function mark_of(agent: string?, harness: string?): string?
	if agent and agent ~= "" then
		return agent
	end
	if harness and string.sub(harness, 1, 4) == "acp." then
		return harness
	end
	return nil
end

-- A model's name as the picker shows it, else its id.
local function model_label(id: string?): string?
	if not id or id == "" then
		return nil
	end
	for _, m in store.catalog:get().models or {} do
		if m.value == id then
			return m.label
		end
	end
	return id
end

local function first_line(text: string?): string?
	if not text then
		return nil
	end
	for line in string.gmatch(text, "[^\n]+") do
		local t = string.match(line, "^%s*(.-)%s*$") or ""
		if t ~= "" then
			return t
		end
	end
	return nil
end

-- A chat's state as a subagent's.
function M.chat_status(c: sessions.Child, now: number): string
	local s = c.status
	if s == "working" or s == "input" or s == "approval" then
		return "running"
	elseif s == "failed" then
		return "failed"
	end
	-- (Made a moment ago and quiet yet: its first turn starting.)
	local fresh = (c :: any).preview == nil and now - c.created < STARTING and (c.updated or c.created) <= c.created
	return if fresh then "running" else "done"
end

-- The session's chat row (its agent, its model), if it has one.
local function row_of(id: string?): sessions.Chat?
	if not id then
		return nil
	end
	for _, c in sessions.now:get().chats do
		if c.id == id then
			return c
		end
	end
	return nil
end

-- A spawn as its row lists it.
local function spawn_item(s: any, parent: sessions.Chat?, times: { [string]: any }, stopping: { [string]: number }): any
	local t = times[s.id] or {}
	local model = model_label(s.model) or (if parent then model_label(parent.model) else nil)
	return {
		id = s.id,
		ref = s.ref,
		title = s.title or locale.s("chat.lanes.subagent"),
		status = s.status or "done",
		mark = if parent then mark_of(parent.agent, parent.harness) else nil,
		model = model,
		step = s.type,
		result = s.result,
		started = t.start,
		ended = t.stop,
		stoppable = true,
		openable = s.ref ~= nil,
		stopping = shared.is_stopping(stopping, `call:{s.id}`),
	}
end

-- A chat its agent made as its subagent, as its row lists it.
local function chat_item(c: sessions.Child, now: number, stopping: { [string]: number }): any
	local status = M.chat_status(c, now)
	local preview = first_line((c :: any).preview)
	return {
		id = `chat:{c.id}`,
		chat = c.id,
		ref = c.id,
		title = c.title,
		status = status,
		mark = mark_of(c.agent, c.harness),
		model = model_label((c :: any).model),
		step = if status == "running" then preview else nil,
		result = if status ~= "running" then preview else nil,
		started = c.created,
		ended = if status ~= "running" then (c.updated or c.created) else nil,
		stoppable = true,
		openable = true,
		stopping = shared.is_stopping(stopping, `chat:{c.id}`),
	}
end

-- Which reply each chat was made in: the last to begin before it was made
-- (else the first), by message id.
function M.assign(messages: { any }, chats: { sessions.Child }): { [string]: { sessions.Child } }
	local out: { [string]: { sessions.Child } } = {}
	if #chats == 0 then
		return out
	end
	local replies = {}
	for _, m in messages do
		if m.role == "assistant" and m.started then
			table.insert(replies, m)
		end
	end
	for _, c in chats do
		local at: string? = nil
		for _, m in replies do
			if m.started <= c.created + SLACK then
				at = m.id
			end
		end
		at = at or (if replies[1] then replies[1].id else nil)
		if at then
			out[at] = out[at] or {}
			table.insert(out[at], c)
		end
	end
	return out
end

function M.use(session: () -> string?): { items: (m: any) -> { any }, stop: (sub: any, settle: (boolean) -> ()) -> (), open: (sub: any) -> () }
	-- The chats made as subagents, by the reply they belong to.
	local made_in = ui.computed(function(): { [string]: { sessions.Child } }
		local sid = session()
		if not sid then
			return {}
		end
		return M.assign(store.messages:get(), sessions.subagent_chats(sid))
	end)
	-- A spawn seen running starts its clock; seen stopped, stops it.
	ui.effect(function()
		local now = os.time()
		local times = TIMES:peek()
		local changed: { [string]: any }? = nil
		for _, m in store.messages:get() do
			for _, s in m.subagents or {} do
				local t = times[s.id]
				if s.status == "running" and not (t and t.start) then
					changed = changed or {}
					;(changed :: any)[s.id] = { start = now }
				elseif s.status ~= "running" and t and t.start and not t.stop then
					changed = changed or {}
					;(changed :: any)[s.id] = { start = t.start, stop = now }
				end
			end
		end
		if changed then
			local all = table.clone(times)
			for id, t in changed do
				all[id] = t
			end
			TIMES:set(all)
		end
	end)
	-- The clock for a chat that's starting: looked at again once it's no
	-- longer fresh (`os.time()` is read where it's needed, not tracked).
	local function items(m: any): { any }
		local out = {}
		local sid = session()
		local parent = row_of(sid)
		local times = TIMES:get()
		-- (Those a stop of all of them is on its way to: "Stopping".)
		local stopping = shared.stopping:get()
		for _, s in m.subagents or {} do
			table.insert(out, spawn_item(s, parent, times, stopping))
		end
		local now = os.time()
		for _, c in made_in:get()[m.id] or {} do
			table.insert(out, chat_item(c, now, stopping))
		end
		return out
	end
	local function stop(sub: any, settle: (boolean) -> ())
		local sid = session()
		if not sid then
			settle(false)
			return
		end
		if sub.chat then
			-- A chat of its own: its turn interrupted, as its own Stop does.
			store.interrupt(sub.chat)
			settle(true)
			return
		end
		store.stop_call(sid, sub.id, sub.title, settle)
	end
	local function open(sub: any)
		local sid = session()
		if not sid then
			return
		end
		if sub.chat then
			lanes.open_chat_subagent(sid, sub.chat, sub.title)
			return
		end
		if sub.ref then
			lanes.open_subagent(sid, { ref = sub.ref, title = sub.title, status = sub.status, call = sub.id })
		end
	end
	return { items = items, stop = stop, open = open }
end

return M

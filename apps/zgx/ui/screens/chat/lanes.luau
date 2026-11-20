--!strict
-- A session's lanes: its side chats and its subagents (production's
-- session_lanes.rs), in a details card at the session page's top right,
-- shown or hidden from the title bar's "Show session details" button (the
-- shell's, screens/shell/title_bar).
--
--   lanes.on()       whether the card exists at all (tracked): off for now
--                    (`chat_lanes_enabled`, false), until how side chats are
--                    reached is settled; its title bar button hides with it.
--                    Subagent tabs (a spawn chip's) and the rows' pill don't
--                    depend on it.
--   lanes.shown()    whether the card shows (a tracked read): the button's
--                    pressed look
--   lanes.toggle()   show it, or hide it
--   lanes.badge()    the open session's count for the button's badge: its
--                    running subagents and its working side chats (tracked)
--   lanes.Overlay()  the card, at the session page's top right, while shown
--
-- What a lane is, and how one opens:
--   lanes.side(id)        a session's side chats (sessions.children), newest first
--   lanes.subagents()     the open chat's subagents, from its transcript's
--                         spawn chips ({ ref, title, status?, at? }), in order
--   lanes.view(slot)      a lane's chat as the host watches it beside the open
--                         one (src/chat.rs `chat.open_lane`): { session,
--                         messages, draft } states; `lanes.watch(slot, id)`
--   lanes.open(session, lane)  a lane in the session's panel, as a tab
--                         ({ kind = "side_chat" | "subagent", id, title })
--   lanes.new_side_chat(session, at?), lanes.fork(session)  the shipping
--                         app's "New side chat" and "Fork chat"; `at`
--                         ({ project?, device? }) where its composer's place
--                         starts instead of its parent's (the panel home's
--                         group "+")

local ds = require("@ds")
local locale = require("@ds/locale")
local SessionLanes = require("@ds/session_lanes")
local sessions = require("@account/sessions")
local route = require("@app/screens/shell/route")
local store = require("@app/screens/chat/store")

local M = {}

-- Shown or not, kept across launches as the shipping app's details are.
M.visible = ui.persist("chat_lanes_shown", false)

-- The card, and its title bar button, at all: off by default (tests turn
-- it on).
M.enabled = ui.state("chat_lanes_enabled", false)

function M.on(): boolean
	return M.enabled:get()
end

function M.shown(): boolean
	return M.enabled:get() and M.visible:get()
end

function M.toggle()
	M.visible:set(not M.visible:peek())
end

-- The open session's row in the sidebar's list, if it has one.
local function row(): sessions.Chat?
	local s = route.session:get()
	if not s then
		return nil
	end
	for _, c in sessions.now:get().chats do
		if c.id == s.id then
			return c
		end
	end
	return nil
end

function M.badge(): number
	local c = row()
	if not c then
		return 0
	end
	return (c.running_subagents or 0) + (c.working_side_chats or 0)
end

function M.side(id: string?): { sessions.Child }
	return sessions.children(id)
end

-- A transcript's subagents: each spawn chip that names its doc, once, in
-- the order they were spawned, with when their turn began. (As the host says
-- them: for a fixture, which has no host.)
function M.subagents_of(messages: { any }): { any }
	local out, seen = {}, {}
	local function walk(tools: { any }?, at: number?)
		for _, t in tools or {} do
			local sub = t.subagent
			if sub and sub.ref and not seen[sub.ref] then
				seen[sub.ref] = true
				table.insert(out, { ref = sub.ref, title = sub.title or locale.s("chat.lanes.subagent"), status = sub.status, at = at })
			end
		end
	end
	local function blocks(list: { any }?, at: number?)
		for _, b in list or {} do
			walk(b.tools, at)
			blocks(b.blocks, at)
		end
	end
	for _, m in messages do
		walk(m.tools, m.started)
		blocks(m.blocks, m.started)
	end
	return out
end

-- (The host says them, so this doesn't read a whole transcript each time it moves.)
local published = ui.shared("chat.subagents", {} :: { any })
local subagents = ui.computed(function(): { any }
	local out: { any } = {}
	for _, s in published:get() do
		table.insert(out, { ref = s.ref, title = s.title or locale.s("chat.lanes.subagent"), status = s.status, at = s.at })
	end
	return out
end)

function M.subagents(): { any }
	return subagents:get()
end

-- A child chat in the tray (one a Zeron chat started): its task's id, the
-- chat's own after this (a subagent's is its doc).
local CHILD = "chat:"

-- The open chat's subagents as the composer's Running tray lists them (its
-- tasks, ds/composer_dock): running first, in the order they were spawned,
-- the finished (done or failed) after, newest first, as the lanes card
-- orders them. A subagent not heard from yet counts as running, as it does
-- there. Each task's id is the subagent's doc (`ref`). With the chat
-- (`session`), its child chats a Zeron chat started (`child_kind`, a
-- subagent's or a side chat's, made through Zeron's tools) are among them,
-- listed as a subagent is; those the person started aren't.
function M.tasks(session: string?): { any }
	local active, finished = {}, {}
	for _, s in subagents:get() do
		local task = { id = s.ref, kind = "agent", name = s.title }
		if s.status == "done" or s.status == "failed" then
			task.ended = s.status
			task.failed = s.status == "failed" or nil
			table.insert(finished, 1, task)
		else
			table.insert(active, task)
		end
	end
	-- (Oldest first, as subagents are spawned; at work, running; else how
	-- it ended, a chat at rest done.)
	local children = table.clone(sessions.children(session))
	table.sort(children, function(a: any, b: any): boolean
		return (a.created or 0) < (b.created or 0)
	end)
	for _, c in children do
		if c.child_kind ~= nil then
			local task = { id = CHILD .. c.id, kind = "agent", name = c.title }
			local st = c.status
			if st == "working" or st == "waiting" or st == "input" then
				table.insert(active, task)
			else
				task.ended = if st == "failed" then "failed" else "done"
				task.failed = st == "failed" or nil
				table.insert(finished, 1, task)
			end
		end
	end
	for _, t in finished do
		table.insert(active, t)
	end
	return active
end

-- A tray row's subagent, opened as a tab of the session's panel; a child
-- chat's, as a side chat's tab, as the panel's own list opens one.
function M.open_task(session: string, ref: string)
	if string.sub(ref, 1, #CHILD) == CHILD then
		local id = string.sub(ref, #CHILD + 1)
		for _, c in sessions.children(session) do
			if c.id == id then
				M.open(session, { kind = "side_chat", id = c.id, title = c.title })
				return
			end
		end
		return
	end
	for _, s in subagents:peek() do
		if s.ref == ref then
			M.open_subagent(session, s)
			return
		end
	end
end

-- A lane's chat, as the host watches it in `slot`.
local views: { [string]: any } = {}
function M.view(slot: string): any
	if not views[slot] then
		views[slot] = {
			session = ui.shared(`chat.lane.{slot}.session`, nil :: any),
			messages = ui.shared(`chat.lane.{slot}.messages`, {} :: { any }),
			draft = ui.shared(`chat.lane.{slot}.draft`, { id = "", text = "" } :: any),
		}
	end
	return views[slot]
end

-- Watch `id` in `slot` (none: let it go). A finished subagent (`frozen`,
-- under `chat`) is read from its upload first, as the shipping app does.
function M.watch(slot: string, id: string?, frozen: boolean?, chat: string?)
	ui.invoke("chat.open_lane", {
		slot = slot,
		id = if id then id else ui.null,
		frozen = frozen == true,
		chat = if chat then chat else ui.null,
	})
end

-- A chat id made here (a side chat's tab, open before the engine has it):
-- version 4's shape, as the engine's own.
function M.new_id(): string
	local function hex(n: number): string
		local out = {}
		for _ = 1, n do
			table.insert(out, string.format("%x", math.random(0, 15)))
		end
		return table.concat(out)
	end
	return `{hex(8)}-{hex(4)}-4{hex(3)}-{string.format("%x", math.random(8, 11))}{hex(3)}-{hex(12)}`
end

-- Where lanes open: the panel's (screens/panel), given by it as it loads,
-- so this module needs no panel to load.
M.opener = nil :: ((session: string, lane: { kind: string, id: string, title: string, data: any? }) -> ())?

function M.open(session: string, lane: { kind: string, id: string, title: string, data: any? })
	local opener = M.opener
	if opener then
		opener(session, lane)
	end
end

-- A subagent's tab: a finished one (done or failed) is read from its
-- upload first.
function M.open_subagent(session: string, sub: any)
	local frozen = sub.status == "done" or sub.status == "failed"
	M.open(session, { kind = "subagent", id = sub.ref, title = sub.title or locale.s("chat.lanes.subagent"), data = { frozen = frozen, parent = session } })
end

-- Side chats opened and not yet sent to, by id: their parent. The engine
-- has nothing of one until its first message (the shipping app's
-- `create_child_chat` writes nothing until then).
M.unsent = ui.state("chat_lanes_unsent", {} :: { [string]: string })
M.take_keys = ui.state("chat_lanes_take_keys", {} :: { [string]: boolean })
-- Where an unsent side chat's composer starts, where it isn't its
-- parent's (screens/chat/lane_tab reads it once, as it seeds the place).
M.starts = ui.state("chat_lanes_starts", {} :: { [string]: { project: string?, device: string? } })

local function set_in(s: State<any>, key: string, value: any)
	local now = table.clone(s:peek())
	now[key] = value
	s:set(now)
end

-- The shipping app's "New side chat": a side chat of the session, opened in
-- its panel; made, where its parent is, as its first message is sent.
function M.new_side_chat(session: string?, at: { project: string?, device: string? }?)
	if not session then
		ds.toast.show { title = locale.s("chat.lanes.need_conversation"), tone = "warning" }
		return
	end
	local id = M.new_id()
	if at and (at.project or at.device) then
		set_in(M.starts, id, { project = at.project, device = at.device })
	end
	set_in(M.unsent, id, session)
	-- (Its composer takes the keys as it's made: nothing to read yet.)
	set_in(M.take_keys, id, true)
	M.open(session, { kind = "side_chat", id = id, title = locale.s("chat.lanes.new_side_chat") })
end

-- A side chat's first message: the chat made, then the message sent to it
-- with what its composer picked (`picks`: model, harness, effort, access).
-- Made where its parent is, unless `place` says otherwise (a new chat's
-- plan, screens/chat/where: project or none, device, branch, folder, a
-- worktree made off the branch as it starts). A promise of nothing, or of
-- why not.
function M.send_first(id: string, text: string, picks: { [string]: any }?, place: { [string]: any }?): any
	local parent = M.unsent:peek()[id]
	local result = { ok = nil :: boolean?, err = nil :: any, waiting = {} :: { any } }
	local promise = {
		after = function(_self: any, done: (any) -> (), failed: ((any) -> ())?)
			if result.ok == true then
				done(nil)
			elseif result.ok == false then
				if failed then
					failed(result.err)
				end
			else
				table.insert(result.waiting, { done = done, failed = failed })
			end
		end,
	}
	local function settle(ok: boolean, err: any)
		result.ok, result.err = ok, err
		for _, w in result.waiting do
			if ok then
				w.done(nil)
			elseif w.failed then
				w.failed(err)
			end
		end
	end
	local made: { [string]: any } = { parent = parent, id = id }
	if place then
		made.project, made.device, made.branch, made.cwd = place.project, place.device, place.branch, place.cwd
		-- (No project: none of the parent's, nor its folder.)
		made.no_project = place.project == nil or nil
	end
	sessions.create(made):after(function()
		set_in(M.unsent, id, nil)
		local req: { [string]: any } = if picks then table.clone(picks) else {}
		req.session, req.text, req.fresh = id, text, true
		if place then
			req.project, req.branch, req.cwd = place.project, place.branch, place.cwd
			req.worktree = place.worktree or nil
		end
		store.send(req :: any):after(function()
			settle(true, nil)
		end, function(err: any)
			settle(false, err)
		end)
	end, function(err: any)
		settle(false, { message = locale.s("chat.lanes.couldnt_create") })
	end)
	return promise
end

-- Forks under way or failed, by the new chat's id: { source, state =
-- "forking" | "failed", error? }. Its tab says so meanwhile.
M.forks = ui.state("chat_lanes_forks", {} :: { [string]: any })

local function start_fork(id: string, source: string)
	set_in(M.forks, id, { source = source, state = "forking" })
	sessions.fork(source, nil, id):after(function()
		set_in(M.forks, id, nil)
	end, function(err: any)
		local why = if type(err) == "table" and err.message then tostring(err.message) else tostring(err)
		set_in(M.forks, id, { source = source, state = "failed", error = why })
	end)
end

-- The shipping app's "Fork chat": a side chat that starts as this one up
-- to its last finished reply, its tab open at once ("Forking…"), with the
-- engine's words for why not and Retry, the same fork again.
function M.fork(session: string?)
	if not session then
		ds.toast.show { title = locale.s("chat.lanes.need_conversation"), tone = "warning" }
		return
	end
	local id = M.new_id()
	M.open(session, { kind = "side_chat", id = id, title = locale.s("chat.lanes.side_chat") })
	start_fork(id, session)
end

function M.retry_fork(id: string)
	local f = M.forks:peek()[id]
	if f then
		start_fork(id, f.source)
	end
end

-- The card, for the open session.
function M.Card(): Node
	local function session(): string?
		local s = route.session:get()
		return if s then s.id else nil
	end
	return SessionLanes {
		session = session,
		side = function(): { any }
			return M.side(session())
		end,
		subagents = function(): { any }
			-- (The open chat's: its transcript is the session shown.)
			local s = store.session:get()
			return if s and s.id == session() then M.subagents() else {}
		end,
		on_new = function()
			M.new_side_chat(session())
		end,
		on_fork = function()
			M.fork(session())
		end,
		on_open_side = function(child: any)
			local s = session()
			if s then
				M.open(s, { kind = "side_chat", id = child.id, title = child.title })
			end
		end,
		on_open_subagent = function(sub: any)
			local s = session()
			if s then
				M.open_subagent(s, sub)
			end
		end,
	}
end

-- The card at the page's top right, while it's shown.
function M.Overlay(): Node
	return ui.show(M.shown, ui.column {
		absolute = true,
		top = ui.raw(12, "the card's inset from the page's corner"),
		right = ui.raw(12, "the card's inset from the page's corner"),
		M.Card(),
	})
end

return M

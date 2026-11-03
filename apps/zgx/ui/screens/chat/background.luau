-- What a chat leaves running with no turn of its own: its background
-- processes (the Running tray's), its subagents still at work (a spawn's
-- call the host lists as running, a chat its agent made as a subagent),
-- and stopping it all at once, as the composer's Stop does once the turn
-- is over and the draft is empty.
--
--   background.items(sid)       what's running, tracked: { key, kind, id, name }
--   background.stop_all(sid)    each of them stopped, on its host; what
--                               couldn't be said in one toast
--   background.stop_task(sid, task, failed)  one process (the tray's Stop)
--   background.busy(sid)        a stop of all of it is on its way (tracked)
--
-- What a stop is on its way to is marked in `shared.stopping` (its key):
-- its row says "Stopping" until it's gone, or until the stop failed (the
-- mark taken off, its Stop back), or for at most `shared.STOP_HOLD`
-- seconds (an answer of "stopping" that never came true gives its Stop
-- back).

local ds = require("@ds")
local store = require("@app/screens/chat/store")
local shared = require("@app/screens/chat/shared")
local sessions = require("@account/sessions")
local Subagents = require("@app/screens/chat/subagents")

local M = {}

export type Kind = "task" | "call" | "chat"
export type Item = { key: string, kind: Kind, id: string, name: string }

local STALE_MS = shared.STOP_HOLD * 1000

-- A stop of all of it on its way, by session (its calls not all answered).
local pending = ui.state("chat_background_pending", {} :: { [string]: boolean })

local function mark(keys: { string }, on: boolean)
	local all = table.clone(shared.stopping:peek())
	local now = os.time()
	for _, k in keys do
		all[k] = if on then now else nil
	end
	shared.stopping:set(all)
end

-- Given a forgotten stop's time: still marked after `STALE_MS` (by this
-- stop, not a later one), its mark goes, the row's Stop back.
local function expire(keys: { string })
	local asked = os.time()
	ui.after(STALE_MS, function()
		local now = shared.stopping:peek()
		local left: { string } = {}
		for _, k in keys do
			if now[k] and now[k] <= asked then
				table.insert(left, k)
			end
		end
		if #left > 0 then
			mark(left, false)
		end
	end)
end

-- What's running in `sid` (the session open in this window), `tracked`
-- or not (a click's read).
local function list(sid: string?, tracked: boolean): { Item }
	local out: { Item } = {}
	if not sid then
		return out
	end
	local s = if tracked then store.session:get() else store.session:peek()
	if not s or s.id ~= sid then
		return out
	end
	for _, t in s.tasks or {} do
		if not t.ended and t.id then
			table.insert(out, { key = `task:{t.id}`, kind = "task", id = t.id, name = t.name or "" })
		end
	end
	local messages = if tracked then store.messages:get() else store.messages:peek()
	for _, m in messages do
		for _, sp in m.subagents or {} do
			if sp.status == "running" and sp.id then
				table.insert(out, { key = `call:{sp.id}`, kind = "call", id = sp.id, name = sp.title or "" })
			end
		end
	end
	local now = os.time()
	for _, c in sessions.subagent_chats(sid) do
		if Subagents.chat_status(c, now) == "running" then
			table.insert(out, { key = `chat:{c.id}`, kind = "chat", id = c.id, name = c.title or "" })
		end
	end
	return out
end

function M.items(sid: string?): { Item }
	return list(sid, true)
end

-- A stop of all of it is on its way: its calls not all answered, or each
-- thing running already asked to stop.
function M.busy(sid: string?): boolean
	if not sid then
		return false
	end
	if pending:get()[sid] then
		return true
	end
	local items = M.items(sid)
	if #items == 0 then
		return false
	end
	local marked = shared.stopping:get()
	for _, it in items do
		if not shared.is_stopping(marked, it.key) then
			return false
		end
	end
	return true
end

local function set_pending(sid: string, on: boolean)
	local all = table.clone(pending:peek())
	all[sid] = if on then true else nil
	pending:set(all)
end

-- Each thing running in `sid` stopped on its host, at once: a process by
-- `StopBackgroundTask`, a spawn by `StopToolCall`, a subagent's chat by an
-- interrupt. Its row says "Stopping" from the click; one that can't be
-- stopped (an agent that can't stop that call alone, a call that failed)
-- gets its Stop back, and those are said together in one toast without
-- holding up the rest.
function M.stop_all(sid: string?)
	if not sid or pending:peek()[sid] then
		return
	end
	local items = list(sid, false)
	if #items == 0 then
		return
	end
	local keys: { string } = {}
	for _, it in items do
		table.insert(keys, it.key)
	end
	mark(keys, true)
	expire(keys)
	set_pending(sid, true)
	local left = #items
	local failed: { string } = {}
	local function settle(it: Item, ok: boolean, reason: string?)
		if not ok then
			mark({ it.key }, false)
			table.insert(failed, if reason and reason ~= "" then `{it.name}: {reason}` else it.name)
		end
		left -= 1
		if left > 0 then
			return
		end
		set_pending(sid, false)
		if #failed > 0 then
			ds.toast.show {
				title = ui.t("chat.stop.some_unsupported", { n = #failed }),
				body = table.concat(failed, "\n"),
				tone = "warning",
			}
		end
	end
	for _, it in items do
		if it.kind == "task" then
			store.stop_task(sid, it.id):after(function()
				-- (False: it had ended already, which is as good.)
				settle(it, true)
			end, function()
				settle(it, false)
			end)
		elseif it.kind == "call" then
			store.ask_stop(sid, it.id):after(function(answer: any)
				local outcome = if type(answer) == "table" then answer.outcome else nil
				if outcome == "stopping" or outcome == "notRunning" then
					settle(it, true)
				else
					local reason = if type(answer) == "table" and type(answer.reason) == "string" then answer.reason else nil
					settle(it, false, reason or ui.t("chat.stop.unsupported"):get())
				end
			end, function()
				settle(it, false)
			end)
		else
			-- A chat of its own: its turn interrupted, as its own Stop does.
			store.interrupt(it.id)
			settle(it, true)
		end
	end
end

-- One background process stopped (the tray's Stop): "Stopping" until it's
-- gone; `failed()` if its host couldn't, its Stop back.
function M.stop_task(sid: string, task: string, failed: () -> ())
	local key = `task:{task}`
	mark({ key }, true)
	expire({ key })
	store.stop_task(sid, task):after(function(stopping: any)
		if stopping ~= true then
			mark({ key }, false)
		end
	end, function()
		mark({ key }, false)
		failed()
	end)
end

return M

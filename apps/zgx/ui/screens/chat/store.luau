-- A chat as the host keeps it (src/chat.rs): the session open in this
-- window, its transcript and what streams into it, the models to choose
-- from, and what can be asked of it. Screens read the states and call the
-- functions; a fixture replaces the functions (and sets the states) as the
-- engine would, so the chat page runs without one.
--
--   store.session:get()   the open session (Session below), nil for none
--   store.messages:get()  its transcript, ds.Transcript's messages, oldest
--                         first, each keyed by `id`
--   store.draft:get()     the reply being written ({ id, text }): it grows
--                         here, a small state, not in the whole list
--   store.catalog:get()   { state, harnesses, models } (ds.ModelPicker's)
--
--   store.open(id?)       watch that session (nil: none), the last one let go
--   store.send(req)       a message to a session: run now, or queued while
--                         a turn runs (a promise). A new chat is made with
--                         sessions.create first, then sent its first
--                         message with `fresh`.
--   store.interrupt(id)   stop its turn
--   store.approve(id, request, scope)  allow what it asked, "once" or "always"
--   store.deny(id, request)
--   store.answer(id, request, answer)  a question's answer ({ values, text })
--   store.configure(id, patch)  { model?, effort?, fast?, options?, access?, plan? }
--   store.queue(id, op)   { op = "remove" | "send" | "move", item, to? }
--   store.queue_edit(id, item), store.queue_finish(id, item, action, text?)
--                         a queued message edited in the composer
--   store.load_catalog()  the models, looked up again
--
-- What can go wrong, a promise's `message`: "offline", "signed_out",
-- "missing" (no such chat), "failed".

local Transcript = require("@ds/transcript")
local ds = require("@ds")
local shared = require("@app/screens/chat/shared")
local prefs = require("@app/screens/settings/prefs")
local route = require("@app/screens/shell/route")

local M = {}

export type Message = Transcript.Message
export type Draft = Transcript.Draft
-- What the agent asks, waiting on an answer: an approval, or a question.
export type Ask = {
	request: string,
	kind: "approval" | "question",
	approval: any?,
	question: any?,
}
export type Session = {
	id: string,
	-- "loading" while its transcript comes; "ready"; "missing" (gone, or
	-- never there); "offline" (nothing to ask).
	state: string,
	title: string?,
	project: string?,
	device: string?,
	-- Its folder on that device.
	cwd: string?,
	harness: string?,
	model: string?,
	effort: string?,
	fast: boolean?,
	-- The choices made of the model's options (option id to choice id).
	options: { [string]: string }?,
	access: string?,
	plan: boolean?,
	-- A turn is running.
	running: boolean?,
	-- How much of the model's context is used, 0 to 1; and what it holds,
	-- as the agent reports it (tokens in it, the window's size).
	context: number?,
	context_tokens: number?,
	context_window: number?,
	ask: Ask?,
	queue: { any }?,
	todos: { any }?,
	tasks: { any }?,
	-- A line about it (a failed send, a run that stopped), or nil.
	notice: any?,
}
export type Catalog = { state: string, harnesses: { ds.Harness }, models: { ds.Model } }
export type Request = {
	-- The session it goes to; nil for a new chat.
	session: string?,
	text: string,
	-- Files and images it carries, by path.
	attachments: { { kind: string, path: string, name: string } }?,
	-- The Appshots among them (screens/chat/appshots): what the host writes
	-- of each after the words.
	appshots: { any }?,
	-- A new chat's: where and how it runs.
	project: string?,
	device: string?,
	checkout: string?,
	branch: string?,
	harness: string?,
	model: string?,
	effort: string?,
	fast: boolean?,
	options: { [string]: string }?,
	access: string?,
	plan: boolean?,
	-- A chat just made (sessions.create): its first message, run in `cwd`,
	-- in a new worktree of it when `worktree`.
	fresh: boolean?,
	cwd: string?,
	worktree: boolean?,
	-- Its id (store.new_id), when the page shows it before it's sent.
	message_id: string?,
}

M.session = ui.shared("chat.session", nil :: Session?)
M.messages = ui.shared("chat.messages", {} :: { Message })
M.draft = ui.shared("chat.draft", { id = "", text = "" } :: Draft)
M.catalog = ui.shared("chat.catalog", { state = "looking", harnesses = {}, models = {} } :: Catalog)
-- A call nothing waits on: its failure lands in the session's notice (the
-- host sets it), so the script needn't hear it.
local function quietly(p: any)
	p:after(function() end, function() end)
end

-- The chat the host is watching for this window ("" for none). Shared, so
-- it outlives a reload as the host's watch does: a page made again asks
-- the host nothing, and only a move to another chat (or letting go, as
-- "Try again" does before asking again) reaches it.
local watching = ui.shared("chat.watching", "")
function M.open(id: string?)
	local key = id or ""
	if watching:peek() == key then
		return
	end
	watching:set(key)
	ui.invoke("chat.open", id)
end

-- A message's id, made here so its prompt shows the moment it's sent
-- (`T.send`) and stays the same row when the engine's copy arrives.
function M.new_id(): string
	local hex = "0123456789abcdef"
	local out = {}
	for i = 1, 32 do
		local n = math.random(1, 16)
		if i == 13 then
			n = 5 -- version 4
		elseif i == 17 then
			n = 9 + (n % 4) -- variant 10xx
		end
		table.insert(out, string.sub(hex, n, n))
	end
	local s = table.concat(out)
	return `{string.sub(s, 1, 8)}-{string.sub(s, 9, 12)}-{string.sub(s, 13, 16)}-{string.sub(s, 17, 20)}-{string.sub(s, 21, 32)}`
end

function M.send(req: Request): any
	return ui.call("chat.send", req)
end

-- A project's branch, as git where it lives has it: whether it's the
-- folder's current one, and the worktree it's checked out in.
export type Ref = { name: string, current: boolean, worktree: string? }

-- A project's branches (the folder at `path`, on `device` when that isn't
-- this one): a promise of its refs, the current first.
function M.refs(path: string, device: string?): any
	return ui.call("chat.refs", { path = path, device = device })
end

-- The clipboard's picture written out as a PNG to send (src/pasted.rs): a
-- promise of its path, nil when the clipboard holds none.
function M.paste_image(): any
	return ui.call("chat.paste_image", nil)
end

-- A prompt that wasn't delivered, sent again.
function M.retry(id: string, message: string)
	quietly(ui.call("chat.retry", { id = id, message = message }))
end

function M.interrupt(id: string)
	quietly(ui.call("chat.interrupt", id))
end

function M.approve(id: string, request: string, scope: string)
	quietly(ui.call("chat.respond", { id = id, request = request, decision = scope }))
end

function M.deny(id: string, request: string)
	quietly(ui.call("chat.respond", { id = id, request = request, decision = "deny" }))
end

-- Back a page in the question being answered: a promise of what was
-- picked on the page shown again (its option values), nil on the first.
function M.question_back(id: string, request: string): any
	return ui.call("chat.question_back", { id = id, request = request })
end

function M.answer(id: string, request: string, answer: { values: { string }, text: string })
	quietly(ui.call("chat.respond", {
		id = id,
		request = request,
		decision = "answer",
		values = answer.values,
		text = answer.text,
	}))
end

function M.configure(id: string, patch: { [string]: any })
	local args = table.clone(patch)
	args.id = id
	quietly(ui.call("chat.configure", args))
end

-- A queued message taken to be edited here (its text comes back, its row
-- says it's being edited); then saved over ("commit") or left ("cancel").
-- Promises: a message being edited elsewhere rejects with "locked", one
-- changed meanwhile with "conflict".
function M.queue_edit(id: string, item: string): any
	return ui.call("chat.queue_edit", { id = id, item = item })
end

function M.queue_finish(id: string, item: string, action: string, text: string?): any
	return ui.call("chat.queue_finish", { id = id, item = item, action = action, text = text })
end

function M.queue(id: string, op: { [string]: any })
	local args = table.clone(op)
	args.id = id
	quietly(ui.call("chat.queue", args))
end

-- Words put into the composer's draft (a plugin's `zeron.compose.insert`,
-- a shortcut): at its end, on a line of their own unless `inline`, and
-- the composer given the keys.
function M.insert(text: string, inline: boolean?)
	local now = shared.text:peek()
	local sep = if now == "" or inline or string.sub(now, -1) == "\n" then "" else "\n"
	shared.text:set(now .. sep .. text)
	shared.focus:add(1)
end

function M.load_catalog()
	ui.invoke("chat.catalog")
end

-- A background process of `session` stopped on its host: a promise of
-- whether it's stopping (false: it had ended already).
function M.stop_task(session: string, task: string): any
	return ui.call("chat.stop_task", { session = session, task = task })
end

-- The device the composer runs on (nil: this one): `M.catalog` is its
-- agents and models from now on (src/chat.rs `catalog_on`).
function M.catalog_on(device: string?)
	ui.invoke("chat.catalog_on", { device = device })
end

-- A bare Escape no one else took (a menu, a dialog, a mention list, a
-- question): with Settings' "Escape stops the agent" on, it stops the
-- chat shown's running turn, as Zeron's does (settings.rs
-- `escape_stops_active_agent`). True if it did.
function M.escape(): boolean
	if not prefs.escape_stops:peek() then
		return false
	end
	local now = route.current:peek()
	local s = M.session:peek()
	if now.name ~= "session" or not s or s.id ~= now.session or not s.running then
		return false
	end
	M.interrupt(s.id)
	return true
end

return M

-- What a chat's composer shows of its session beyond its draft, and what's
-- done with it, for one chat by its id: what the agent asks (a question,
-- an approval), the queue, the todos and the background tasks (the tray).
-- The window's composer (the chat shown) and a split pane's (its own chat,
-- focused or not) are made from the same, so a chat shows the same in
-- either, and a pane out of focus is as live as the one in it.
--
--   local a = Asks.bind { id = fn, session = fn, chosen = state, asked = state }
--   ds.Composer { question = a.question, approval = a.approval, chosen = a.chosen,
--     on_pick = a.on_pick, on_answer = a.on_answer, …, queue = a.queue, todos = a.todos,
--     tasks = a.tasks }
--
-- `session` is the chat's record as the host publishes it (the window's
-- chat's, or a lane's); `chosen` the options picked in its question, which
-- start afresh as another question is asked (`asked`, the request they're
-- for, and its chat).

local store = require("@app/screens/chat/store")
local Carried = require("@app/screens/chat/carried")
local T = require("@ds/transcript")

local M = {}

-- A queued message edited in a chat's composer, by whichever composer shows
-- that chat: `text` its draft, `editing` which message and the draft it set
-- aside (put back as the edit ends), `focus` gives it the keys, `say` a
-- notice. `cargo`/`set_cargo`: the draft's chips, where the message's
-- files show while it's edited (the draft's own set aside meanwhile).
export type Edits = {
	id: () -> string?,
	text: State<string>,
	editing: State<{ item: string, aside: string, aside_cargo: { any }? }?>,
	focus: () -> (),
	say: (tone: string, key: string) -> (),
	cargo: (() -> { any })?,
	set_cargo: (({ any }) -> ())?,
}

local chips = 0
-- A queued message's files (the host's `queued_attachment`s) as chips.
local function chips_of(attachments: { any }?): { any }
	local out = {}
	for _, a in attachments or {} do
		chips += 1
		table.insert(out, {
			id = `queued-file-{chips}`,
			kind = a.kind,
			name = a.name,
			src = a.src,
			path = a.path,
			on_host = true,
		})
	end
	return out
end

function M.queue_edits(e: Edits): { [string]: any }
	-- ("commit" saves what's in the draft, "cancel" leaves it as it was.)
	local function finish(action: string)
		local now = e.editing:peek()
		local id = e.id()
		if not now or not id then
			return
		end
		local words = e.text:peek()
		-- The files it carries now: its chips, as edited.
		local files = if e.cargo then Carried.pack(e.cargo(), "", "").attachments else nil
		ui.batch(function()
			e.editing:set(nil)
			e.text:set(now.aside)
			if e.set_cargo and now.aside_cargo then
				e.set_cargo(now.aside_cargo)
			end
		end)
		store.queue_finish(id, now.item, action, if action == "commit" then words else nil, if action == "commit" then files else nil):after(function() end, function(err: any)
			local why = err and err.message
			e.say("danger", if why == "conflict" then "chat.composer.edit_conflict" else "chat.composer.edit_not_saved")
		end)
	end
	return {
		finish = finish,
		on_queue_edit = function(item: string)
			local id = e.id()
			if not id or e.editing:peek() then
				return
			end
			store.queue_edit(id, item):after(function(got: any)
				ui.batch(function()
					e.editing:set { item = item, aside = e.text:peek(), aside_cargo = if e.cargo then e.cargo() else nil }
					e.text:set(if got then got.text else "")
					if e.set_cargo then
						e.set_cargo(chips_of(if got then got.attachments else nil))
					end
				end)
				e.focus()
			end, function(err: any)
				e.say("warning", if err and err.message == "locked" then "chat.composer.edit_locked" else "chat.composer.edit_unavailable")
			end)
		end,
		on_queue_save = function()
			finish("commit")
		end,
		on_queue_cancel = function()
			finish("cancel")
		end,
	}
end

export type Bind = {
	id: () -> string?,
	session: () -> any,
	chosen: State<{ [string]: boolean }>,
	asked: State<any>,
	-- A notice, when a queued message's send or steer doesn't go.
	say: ((tone: string, key: string) -> ())?,
	-- Its transcript's name (ds/transcript's: a side chat's); none, the
	-- chat page's. (A function: the one it names as it steers, the window's
	-- composer's following the pane at its foot.)
	name: (string | () -> string?)?,
}

function M.bind(b: Bind): { [string]: any }
	local ask = ui.computed(function(): store.Ask?
		local s = b.session()
		return if s then s.ask else nil
	end)
	local question = ui.computed(function(): any
		local a = ask:get()
		return if a and a.kind == "question" then a.question else nil
	end)
	local approval = ui.computed(function(): any
		local a = ask:get()
		return if a and a.kind == "approval" then a.approval else nil
	end)
	-- (Fresh for another question of the same chat: moving to another
	-- chat brings its own picks with its draft, screens/chat/shared.)
	ui.effect(function()
		local a = ask:get()
		local now = { id = b.id(), request = if a then a.request else nil }
		local was = b.asked:peek()
		if type(was) ~= "table" or was.id ~= now.id or was.request ~= now.request then
			b.asked:set(now)
			if type(was) == "table" and was.id == now.id then
				b.chosen:set({})
			end
		end
	end)
	local function list(key: string): any
		return ui.computed(function(): { any }
			local s = b.session()
			return if s and s[key] then s[key] else {}
		end)
	end
	local queue = list("queue")
	-- Each act is on the chat and the question it was asked about, as they
	-- are as it's taken.
	local function on_ask(f: (id: string, request: string) -> ())
		return function()
			local id, a = b.id(), ask:peek()
			if id and a then
				f(id, a.request)
			end
		end
	end
	return {
		ask = ask,
		question = question,
		approval = approval,
		chosen = b.chosen,
		on_pick = function(value: string)
			local q = question:peek()
			local now = table.clone(b.chosen:peek())
			if q and q.multi then
				now[value] = not now[value] or nil
			else
				now = { [value] = true }
			end
			b.chosen:set(now)
		end,
		on_answer = function(answer: any)
			local id, a = b.id(), ask:peek()
			if id and a then
				store.answer(id, a.request, answer)
			end
		end,
		on_close_question = on_ask(store.deny),
		-- Back a page: it shows again with what was picked on it.
		on_back = on_ask(function(id: string, request: string)
			store.question_back(id, request):after(function(picked: any)
				local now: { [string]: boolean } = {}
				for _, v in picked or {} do
					now[v] = true
				end
				b.chosen:set(now)
			end, function() end)
		end),
		-- (The engine allows once, each time it's asked.)
		can_always = false,
		on_approve = function(scope: string)
			local id, a = b.id(), ask:peek()
			if id and a then
				store.approve(id, a.request, scope)
			end
		end,
		on_deny = on_ask(store.deny),
		queue = queue,
		on_queue_send = function(item: string)
			local id = b.id()
			if id then
				-- Its prompt glides to the top and holds the runway, as a
				-- sent one does (the host's copy has the queued row's id).
				T.steer(function()
					store.queue(id, { op = "send", item = item }, function()
						if b.say then
							b.say("danger", "chat.composer.didnt_send")
						end
					end)
				end, item, if type(b.name) == "function" then b.name() else b.name)
			end
		end,
		on_queue_remove = function(item: string)
			local id = b.id()
			if id then
				store.queue(id, { op = "remove", item = item })
			end
		end,
		on_queue_reorder = function(from: number, to: number)
			local id = b.id()
			local items = queue:peek()
			if id and items[from] then
				store.queue(id, { op = "move", item = items[from].id, to = to })
			end
		end,
		todos = list("todos"),
		tasks = list("tasks"),
	}
end

return M

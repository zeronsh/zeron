-- Orchestration as the host keeps it (src/orchestration.rs): orchestrators,
-- their crews, the board, what's asked of you and the Brain dump's dumps
-- (`WatchOrchestration`), what passes between agents (`WatchComms`) and how
-- busy each device is (`WatchDeviceLoads`, only while something shows it).
-- The shape is docs/design/orchestrators.md's "Bridge";
-- account/orchestration_sample.luau is a whole one.
--
--   orchestration.now:get()      { state = "loading" | "ready", orchestrators,
--                                  crew, items, asks, dumps }
--   orchestration.loads:get()    { [device id] = { cpu?, mem_used?, mem_total?,
--                                  load1?, cores?, online, at } }
--   orchestration.comms:get()    { { id, kind, from, to, summary, at } },
--                                newest last, at most 100
--   orchestration.loads_wanted(on)  a screen showing loads starts (true) or
--                                stops (false) wanting them; the host
--                                samples only while one does
--   orchestration.op(op)         one `Orchestrate` op, as the wire has it
--                                (`{ op = "dump", text }`): a promise of its
--                                reply
--   orchestration.dump(text)     the Brain dump takes it: a promise of its id
--   orchestration.answer(id, { option?, text? }, ask?)  an ask answered
--                                (option 1-based); orchestration.answer_ask
--                                on the host, else the `answerAsk` op in
--                                plain words (`ask`, or its option's label,
--                                gives them); fails after TIMEOUT_MS
--   orchestration.defer(id, until_ms?)  an ask put off (nil: back now);
--                                orchestration.defer_ask on the host, else
--                                the `deferAsk` op
--   orchestration.reason(err)    a failure's words
--   orchestration.dismiss(id)    an ask put away
--   orchestration.reroute(dump, route, target)  a route moved (or an unsure
--                                one placed): `target` as a route's
--                                (`{ kind = "orchestrator", id }`)
--   orchestration.move_item(id, state, order?)  a card moved on the board
--   orchestration.approve_item(id)              a card in Review approved
--   orchestration.request_changes(id, text)     changes asked for on one
--   orchestration.set_priority(id, priority)
--   orchestration.paused_card(chat)  the paused card it's a thread of, if any
--   orchestration.settings:get()  the profile's settings, synced and read by
--                                every engine: { crew_auto_archive,
--                                crew_idle_hours } (defaults until the
--                                engine says: on, 4)
--   orchestration.auto_archive   a state for a Switch: crew threads left
--                                idle archived (writes `updateSettings`)
--   orchestration.idle_hours     a state for a Select: how long idle first
--   orchestration.IDLE_HOURS     the windows offered (hours)
--
-- Chats come from `sessions.now` (account/sessions); their rows carry
-- `activity = { kind, label, since }` and `todo = { done, total, current? }`.
--
-- How far each chat's work has got (`WatchProgress`,
-- src/orchestration_progress.rs), read and never asked of the agent:
--
--   orchestration.progress:get() { [chat id] = { state, device_id, started?,
--                                  last_step?, step?, steps? = { done, total },
--                                  note?, note_at?, stalled, tool?,
--                                  tool_since?, refreshing, eta_secs?,
--                                  updated } }
--   orchestration.check(chat)    its note made fresh now: a promise of its
--                                row (`refreshing` while the note is on its
--                                way)

local M = {}

M.now = ui.shared("orchestration", {
	state = "loading",
	orchestrators = {},
	crew = {},
	items = {},
	asks = {},
	dumps = {},
} :: any)
M.loads = ui.shared("orchestration.loads", {} :: { [string]: any })
M.comms = ui.shared("orchestration.comms", {} :: { any })

-- Its parts, each followed on its own.
M.orchestrators = ui.computed(function(): { any }
	return M.now:get().orchestrators or {}
end)
M.dumps = ui.computed(function(): { any }
	return M.now:get().dumps or {}
end)
M.asks = ui.computed(function(): { any }
	return M.now:get().asks or {}
end)

function M.op(op: { [string]: any }): any
	return ui.call("orchestration.op", op)
end

-- The profile's settings (src/orchestration.rs `settings`). What was just
-- asked for shows at once, until the engine's state says it (or the ask
-- fails).
local DEFAULT_SETTINGS = { crew_auto_archive = true, crew_idle_hours = 4 }
local asked = ui.state("orchestration_settings_asked", {} :: { [string]: any })
M.settings = ui.computed(function(): { crew_auto_archive: boolean, crew_idle_hours: number }
	local s = M.now:get().settings or DEFAULT_SETTINGS
	local a = asked:get()
	local out = table.clone(s)
	for k, v in a do
		out[k] = v
	end
	return out :: any
end)
-- (What the engine now says puts an ask to rest.)
ui.effect(function()
	local s = M.now:get().settings
	local a = asked:peek()
	if not s then
		return
	end
	local left, settled = {}, false
	for k, v in a do
		if s[k] == v then
			settled = true
		else
			left[k] = v
		end
	end
	if settled then
		asked:set(left)
	end
end)

M.IDLE_HOURS = { 1, 4, 6, 12, 24, 72, 168 }

-- `key` (snake_case) set to `value` through `updateSettings`.
local function set_setting(key: string, value: any)
	local a = table.clone(asked:peek())
	a[key] = value
	asked:set(a)
	M.op({ op = "updateSettings", [M.camel(key)] = value }):after(function() end, function()
		local back = table.clone(asked:peek())
		back[key] = nil
		asked:set(back)
	end)
end

-- A state for `key` (a Switch's, a Select's): what the settings say, and a
-- change of the person's sent to the engine. (Each follows the other: what
-- the engine says is set here; what's set here and isn't what it says is
-- asked of it.)
local function setting_state(key: string): any
	local st = ui.state(`orchestration_setting_{key}`, (DEFAULT_SETTINGS :: any)[key])
	ui.effect(function()
		local v = (M.settings:get() :: any)[key]
		if st:peek() ~= v then
			st:set(v)
		end
	end)
	ui.effect(function()
		local v = st:get()
		if (M.settings:peek() :: any)[key] ~= v then
			set_setting(key, v)
		end
	end)
	return st
end
M.auto_archive = setting_state("crew_auto_archive")
M.idle_hours = setting_state("crew_idle_hours")

-- Wanting loads, counted by the host (several screens may show them).
function M.loads_wanted(on: boolean)
	ui.invoke("orchestration.loads_wanted", on)
end

-- A dump: its words, and the files they carry (as the chat composer packs
-- a message's: `{ kind, path, name, on_host }`), sent on toward the Brain
-- dump's chat (`chat`, its id) as a message's are (src/orchestration.rs);
-- `picks`, the composer's (as a chat send's), become the op's `config`.
-- `refs`: what in the Control room it references (ds/orch_ref, dragged onto
-- Capture: `{ kind, id, label, context? }`), passed on to the engine as is.
function M.dump(text: string, attachments: { any }?, chat: string?, picks: { [string]: any }?, refs: { any }?): any
	local op: { [string]: any } = { op = "dump", text = text }
	op.chat = chat
	if attachments and #attachments > 0 then
		op.attachments = attachments
	end
	if refs and #refs > 0 then
		local list = {}
		for _, r in refs do
			table.insert(list, { kind = r.kind, id = r.id, label = r.label or "", context = r.context })
		end
		op.refs = list
	end
	-- The Brain dump's config as the composer has it picked (its model,
	-- agent, effort, Fast and options), applied to its chat before the run:
	-- the chat send's shape, made the engine's by the bridge.
	if picks and picks.model then
		op.picks = picks
	end
	return M.op(op)
end

-- A host action called (`ui.call`): kept apart so the fixtures can stand
-- in for the host (one without an action, one whose engine refuses).
function M.host(name: string, arg: any): any
	return ui.call(name, arg)
end

-- How long an answer or a defer waits on the engine before it's said to
-- have failed.
M.TIMEOUT_MS = 15000

-- What went wrong, in words: a promise's error (a `UiError`'s `message`),
-- or what a call raised, its script position gone.
function M.reason(err: any): string
	if type(err) == "string" then
		return (string.gsub(err, "^.-:%d+: ", ""))
	end
	if type(err) == "table" or type(err) == "userdata" then
		local ok, message = pcall(function(): any
			return (err :: any).message
		end)
		if ok and type(message) == "string" then
			return message
		end
	end
	return tostring(err)
end

-- Whether a call raised because the host has no such action (an app built
-- before it).
local function missing(err: any): boolean
	return string.find(M.reason(err), "no host action named", 1, true) ~= nil
end

-- A promise of our own (its `:after` as `ui.call`'s), settled once: by the
-- engine's reply, its error, or the timeout (`{ kind = "timeout" }`).
local function promise(): any
	local p: any = { done = false }
	type Waiter = { on_value: ((any) -> ())?, on_error: ((any) -> ())? }
	local waiting: { Waiter } = {}
	local stop: (() -> ())? = nil
	local function settle(ok: boolean, v: any)
		if p.done then
			return
		end
		p.done, p.ok, p.value = true, ok, v
		p.error = if ok then nil else v
		local cancel = stop
		stop = nil
		if cancel then
			cancel()
		end
		for _, w in waiting do
			local f = if ok then w.on_value else w.on_error
			if f then
				f(v)
			end
		end
		table.clear(waiting)
	end
	function p.after(self: any, on_value: ((any) -> ())?, on_error: ((any) -> ())?): any
		if self.done then
			local f = if self.ok then on_value else on_error
			if f then
				f(self.value)
			end
		else
			table.insert(waiting, { on_value = on_value, on_error = on_error })
		end
		return self
	end
	p.resolve = function(v: any)
		settle(true, v)
	end
	p.reject = function(e: any)
		settle(false, e)
	end
	stop = ui.after(M.TIMEOUT_MS, function()
		stop = nil
		settle(false, { kind = "timeout", message = "timeout" })
	end)
	return p
end

-- `call`'s promise, or (nil and) what it raised.
local function attempt(call: () -> any): (any?, any?)
	local ok, p = pcall(call)
	if ok then
		return p, nil
	end
	return nil, p
end

-- An answer as plain words, for an engine that takes only words: the
-- option's label, and " — <words>" when there are words too. `about` is
-- the ask (its options give the label) or the label itself.
function M.plain_answer(about: any, o: { option: number?, text: string? }): string
	local label: string? = nil
	if type(about) == "string" then
		label = about
	elseif type(about) == "table" and o.option then
		local opt = (about.options or {})[o.option]
		label = opt and opt.label
	end
	if label == "" then
		label = nil
	end
	local words = if o.text then string.match(o.text, "^%s*(.-)%s*$") or "" else ""
	if label and words ~= "" then
		return `{label} — {words}`
	end
	return label or words
end

-- Whether the engine turned the option down (one from before options: an
-- unknown field, or an empty answer when only the option was sent).
local function refuses_option(err: any): boolean
	local r = string.lower(M.reason(err))
	return string.find(r, "option", 1, true) ~= nil or string.find(r, "empty", 1, true) ~= nil
end

-- An ask answered: an option of it (1-based, as Luau counts), words of
-- the person's, or both (one of them, at least). Needs you, a board card,
-- the decision overlay and the floating composer all answer through here.
-- (A plain string is taken as the words.) `about`, the ask or its
-- option's label, gives the plain words an older engine takes instead. A
-- promise of the engine's `{ id }`, rejected with what went wrong (or
-- `{ kind = "timeout" }` past TIMEOUT_MS).
--
-- An app without `orchestration.answer_ask` sends the `answerAsk` op, the
-- answer as plain words; an engine that refuses the option (or says the
-- answer is empty) is asked once more, with the plain words.
function M.answer(ask_id: string, opts: { option: number?, text: string? } | string, about: any?): any
	local o: { option: number?, text: string? } = if type(opts) == "string" then { text = opts } else opts
	local plain = M.plain_answer(about, o)
	local out = promise()
	local function by_op(err: any)
		if plain == "" then
			out.reject(err)
			return
		end
		local p, raised = attempt(function(): any
			return M.op({ op = "answerAsk", id = ask_id, answer = plain })
		end)
		if not p then
			out.reject(raised)
			return
		end
		p:after(out.resolve, out.reject)
	end
	local function by_host(call: { [string]: any }, again: boolean)
		local p, raised = attempt(function(): any
			return M.host("orchestration.answer_ask", call)
		end)
		if not p then
			if missing(raised) then
				by_op(raised)
			else
				out.reject(raised)
			end
			return
		end
		p:after(out.resolve, function(e: any)
			if again and call.option and plain ~= "" and refuses_option(e) then
				by_host({ id = ask_id, text = plain }, false)
			else
				out.reject(e)
			end
		end)
	end
	local call: { [string]: any } = { id = ask_id }
	call.option = o.option
	if o.text and o.text ~= "" then
		call.text = o.text
	end
	by_host(call, true)
	return out
end

-- An ask put off until `until_ms` (Unix milliseconds): out of Needs you,
-- "Deferred" on its card, back (and said again) when the time comes. No
-- time: back now. A promise of the engine's `{ id }`, rejected as
-- `answer`'s is. An app without `orchestration.defer_ask` sends the
-- `deferAsk` op; an engine without that op says so, and that's the error.
function M.defer(ask_id: string, until_ms: number?): any
	local secs = if until_ms then math.floor(until_ms / 1000) else nil
	local out = promise()
	local call: { [string]: any } = { id = ask_id }
	call["until"] = secs
	local p, raised = attempt(function(): any
		return M.host("orchestration.defer_ask", call)
	end)
	if not p and missing(raised) then
		local op: { [string]: any } = { op = "deferAsk", id = ask_id }
		op["until"] = if secs then os.date("!%Y-%m-%dT%H:%M:%SZ", secs) else nil
		p, raised = attempt(function(): any
			return M.op(op)
		end)
	end
	if not p then
		out.reject(raised)
	else
		p:after(out.resolve, out.reject)
	end
	return out
end

function M.dismiss(ask_id: string): any
	return M.op({ op = "dismissAsk", id = ask_id })
end

-- A snake_case word as the wire spells it (`new_thread` → `newThread`).
local function camel(word: string): string
	return (string.gsub(word, "_(%l)", string.upper))
end
M.camel = camel

-- (A new thread or orchestrator not made yet goes with an empty id: the
-- wire's `newThread`/`newOrchestrator` carry one, and the engine makes it.)
local NEEDS_ID: { [string]: boolean } = { orchestrator = true, chat = true, new_orchestrator = true, new_thread = true, item = true }
function M.reroute(dump_id: string, route_id: string, target: { kind: string, id: string? }): any
	local id = target.id
	if id == nil and NEEDS_ID[target.kind] then
		id = ""
	end
	return M.op({ op = "reroute", dumpId = dump_id, routeId = route_id, target = { kind = camel(target.kind), id = id } })
end

function M.move_item(id: string, state: string, order: number?): any
	return M.op({ op = "upsertItem", item = { id = id, state = state, order = order } })
end

-- A card paused by the person: its threads stopped, their queued messages
-- held, the card Blocked in Paused. A promise.
function M.pause_item(id: string): any
	return M.op({ op = "pauseItem", id = id })
end

-- A paused card resumed into `state` (left out: Now). A promise.
function M.resume_item(id: string, state: string?): any
	return M.op({ op = "resumeItem", id = id, state = if state then camel(state) else nil })
end

-- A card in Review approved: Done, its approval recorded. A promise.
function M.approve_item(id: string): any
	return M.op({ op = "approveItem", id = id })
end

-- Changes asked for on a card in Review: `text` to its thread, the card
-- back in Now. A promise.
function M.request_changes(id: string, text: string): any
	return M.op({ op = "requestChanges", id = id, text = text })
end

-- What the person does to a card on the board, where the engine works its
-- place out: marked done (or dropped), opened again, kept for later (or
-- let go), and opened (seen: its dot gone). Each a promise.
function M.close_item(id: string, dropped: boolean?): any
	return M.op({ op = "closeItem", id = id, dropped = if dropped then true else nil })
end

function M.reopen_item(id: string): any
	return M.op({ op = "reopenItem", id = id })
end

function M.hold_item(id: string, held: boolean): any
	return M.op({ op = "holdItem", id = id, held = held })
end

function M.seen_item(id: string): any
	return M.op({ op = "seenItem", id = id })
end

-- A new task from the board: a dump (toward the Brain dump's `chat`) that
-- names the orchestrator it's for, when one was chosen; else routed as any
-- dump is.
function M.new_task(text: string, orchestrator: { id: string, name: string? }?, chat: string?): any
	local refs = if orchestrator then { { kind = "orchestrator", id = orchestrator.id, label = orchestrator.name or "" } } else nil
	return M.dump(text, nil, chat, nil, refs)
end

function M.set_priority(id: string, priority: string): any
	return M.op({ op = "upsertItem", item = { id = id, priority = priority } })
end

-- A chat put on the board: its card (the one it has when one isn't done
-- yet, else a new one). A promise of the card's `{ id }`.
function M.add_to_board(chat_id: string): any
	return M.op({ op = "addToBoard", chatId = chat_id })
end

-- A Planned card started: its thread made (in its project), the card
-- Running. A promise. (Through `startItem`: the engine works a card's
-- place out, and `state` on `upsertItem` is going.)
function M.start_item(id: string): any
	return M.op({ op = "startItem", id = id })
end

-- A card taken off the board (kept, marked removed; its chat untouched).
function M.remove_item(id: string): any
	return M.op({ op = "removeItem", id = id })
end

-- What the person did to a card that put it where it was, done again on
-- the card made in its place (`restore_item`): the op, or nil. (`state` is
-- the column the engine still writes for older readers, read here, never
-- set: the engine works the new card's place out from these.)
local function settle_op(item: any, id: string): { [string]: any }?
	local state = item.state
	if state == "done" then
		return { op = "closeItem", id = id }
	elseif state == "dropped" then
		return { op = "closeItem", id = id, dropped = true }
	elseif state == "inbox" then
		return { op = "holdItem", id = id, held = true }
	elseif M.is_paused(item) then
		return { op = "pauseItem", id = id }
	end
	return nil
end
M.settle_op = settle_op

-- A card taken off the board, put back as it was: made again from its
-- fields (the engine brings none back), a new card, then closed, held or
-- paused as it was. A promise of `{ id }`.
function M.restore_item(item: any): any
	local src = item.source
	local links: { any } = {}
	for _, l in item.links or {} do
		table.insert(links, { kind = l.kind, label = l.label, target = l.target })
	end
	local out = promise()
	local made, raised = attempt(function(): any
		return M.op({
			op = "upsertItem",
			item = {
				orchestratorId = item.orchestrator_id,
				title = item.title,
				detail = item.detail or "",
				priority = item.priority,
				progress = item.progress,
				threads = item.threads or {},
				links = links,
				outcome = item.outcome,
				source = if src and src.dump_id then { dumpId = src.dump_id, routeId = src.route_id or "" } else nil,
				order = item.order,
			},
		})
	end)
	if not made then
		out.reject(raised)
		return out
	end
	made:after(function(reply: any)
		local id = type(reply) == "table" and reply.id
		local then_op = if type(id) == "string" then settle_op(item, id) else nil
		if not then_op then
			out.resolve(reply)
			return
		end
		local p, err = attempt(function(): any
			return M.op(then_op)
		end)
		if not p then
			out.reject(err)
			return
		end
		p:after(function()
			out.resolve(reply)
		end, out.reject)
	end, out.reject)
	return out
end

-- Whether a chat has a card on the board not done yet.
function M.on_board(chat_id: string): boolean
	for _, it in M.now:peek().items or {} do
		if it.state ~= "done" then
			for _, t in it.threads or {} do
				if t == chat_id then
					return true
				end
			end
		end
	end
	return false
end

-- Whether a card is paused by the person: Blocked, `paused_at` set (a
-- number or a date; false or empty once resumed).
function M.is_paused(it: any): boolean
	if type(it) ~= "table" or it.state ~= "blocked" or it.removed then
		return false
	end
	local p = it.paused_at
	if p == nil then
		p = it.pausedAt
	end
	return type(p) == "number" or (type(p) == "string" and p ~= "")
end

-- The paused card a chat is a thread of, if any: while it's paused, what's
-- sent to the chat is held in its queue. (Followed: read in a computed, it
-- changes as the board does.)
function M.paused_card(chat_id: string?): any
	if not chat_id then
		return nil
	end
	for _, it in M.now:get().items or {} do
		if M.is_paused(it) and table.find(it.threads or {}, chat_id) then
			return it
		end
	end
	return nil
end

-- A dump asked to be shown (its id): the Control room brings its receipt
-- into view, opened, and clears it. Set by a board card's source, from any
-- page (the page goes to the Control room as it sets it).
M.reveal_dump = ui.state("orchestration_reveal_dump", nil :: string?)

-- How far each chat's work has got, by chat id.
M.progress = ui.shared("orchestration.progress", {} :: { [string]: any })

function M.check(chat_id: string): any
	return ui.call("orchestration.progress_check", chat_id)
end

return M

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
--   orchestration.answer(id, text)   an ask answered
--   orchestration.dismiss(id)    an ask put away
--   orchestration.reroute(dump, route, target)  a route moved (or an unsure
--                                one placed): `target` as a route's
--                                (`{ kind = "orchestrator", id }`)
--   orchestration.move_item(id, state, order?)  a card moved on the board
--   orchestration.set_priority(id, priority)
--   orchestration.settings:get()  the profile's settings, synced and read by
--                                every engine: { crew_auto_archive,
--                                crew_idle_hours } (defaults until the
--                                engine says: on, 12)
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
local DEFAULT_SETTINGS = { crew_auto_archive = true, crew_idle_hours = 12 }
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

M.IDLE_HOURS = { 1, 6, 12, 24, 72, 168 }

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
function M.dump(text: string, attachments: { any }?, chat: string?, picks: { [string]: any }?): any
	local op: { [string]: any } = { op = "dump", text = text }
	op.chat = chat
	if attachments and #attachments > 0 then
		op.attachments = attachments
	end
	-- The Brain dump's config as the composer has it picked (its model,
	-- agent, effort, Fast and options), applied to its chat before the run:
	-- the chat send's shape, made the engine's by the bridge.
	if picks and picks.model then
		op.picks = picks
	end
	return M.op(op)
end

function M.answer(ask_id: string, text: string): any
	return M.op({ op = "answerAsk", id = ask_id, answer = text })
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

function M.set_priority(id: string, priority: string): any
	return M.op({ op = "upsertItem", item = { id = id, priority = priority } })
end

-- A chat put on the board: its card (the one it has when one isn't done
-- yet, else a new one). A promise of the card's `{ id }`.
function M.add_to_board(chat_id: string): any
	return M.op({ op = "addToBoard", chatId = chat_id })
end

-- An Inbox or Next card started: its thread made (in its project), the
-- card in Now; at `order` when given (where it was dropped: the person
-- moving a card with no thread to Now starts it). A promise.
function M.start_item(id: string, order: number?): any
	if order then
		return M.op({ op = "upsertItem", item = { id = id, state = "active", order = order } })
	end
	return M.op({ op = "startItem", id = id })
end

-- A card taken off the board (kept, marked removed; its chat untouched).
function M.remove_item(id: string): any
	return M.op({ op = "removeItem", id = id })
end

-- A card taken off the board, put back as it was: made again from its
-- fields (the engine brings none back), a new card. A promise of `{ id }`.
function M.restore_item(item: any): any
	local src = item.source
	local links: { any } = {}
	for _, l in item.links or {} do
		table.insert(links, { kind = l.kind, label = l.label, target = l.target })
	end
	return M.op({
		op = "upsertItem",
		item = {
			orchestratorId = item.orchestrator_id,
			title = item.title,
			detail = item.detail or "",
			state = item.state,
			priority = item.priority,
			progress = item.progress,
			threads = item.threads or {},
			links = links,
			outcome = item.outcome,
			source = if src and src.dump_id then { dumpId = src.dump_id, routeId = src.route_id or "" } else nil,
			order = item.order,
		},
	})
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

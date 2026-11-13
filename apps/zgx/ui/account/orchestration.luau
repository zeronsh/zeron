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

-- Wanting loads, counted by the host (several screens may show them).
function M.loads_wanted(on: boolean)
	ui.invoke("orchestration.loads_wanted", on)
end

-- A dump: its words, and the files they carry (as the chat composer packs
-- a message's: `{ kind, path, name, on_host }`), sent on toward the Brain
-- dump's chat (`chat`, its id) as a message's are (src/orchestration.rs).
function M.dump(text: string, attachments: { any }?, chat: string?): any
	local op: { [string]: any } = { op = "dump", text = text }
	if attachments and #attachments > 0 then
		op.attachments = attachments
		op.chat = chat
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

-- How far each chat's work has got, by chat id.
M.progress = ui.shared("orchestration.progress", {} :: { [string]: any })

function M.check(chat_id: string): any
	return ui.call("orchestration.progress_check", chat_id)
end

return M

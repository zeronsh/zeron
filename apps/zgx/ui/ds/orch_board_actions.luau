--!strict
-- What the person does to a card, for every part of the board that offers
-- it (a card's menu, its detail, a row of the list): each sent through its
-- host's prop (ds/orch_board_ctx), said in a toast once it took (a move
-- the person caused says so; one the engine made never does), and said in
-- the danger's tone, with why, when it was refused.
--
--   local act = Actions.make(ctx)
--   act.close(entry, dropped), act.reopen(entry), act.hold(entry, held),
--   act.pause(entry), act.resume(entry), act.start(entry), act.remove(entry),
--   act.request_changes(entry, text), act.snooze(ask, until_ms?),
--   act.answer(ask, choice), act.expand(ask), act.open_thread(entry, chat?)
--   act.menu(entry) -> { ContextMenu.Entry }
--   act.snooze_menu(ask) -> { ContextMenu.Entry }

local locale = require("@self/locale")
local toast = require("@self/toast")
local decide = require("@self/orch_decide")
local Ctx = require("@self/orch_board_ctx")

local M = {}

-- How long a delete waits for its Undo (its toast's while), and how long
-- after that its card stays hidden for the engine's rows to leave it out.
M.UNDO_MS = 6000
M.LAND_MS = 4000

export type Actions = {
	reason: (err: any) -> string,
	can: (what: string) -> boolean,
	open_thread: (e: Ctx.Entry, chat: string?) -> (),
	close: (e: Ctx.Entry, dropped: boolean) -> (),
	reopen: (e: Ctx.Entry) -> (),
	hold: (e: Ctx.Entry, held: boolean) -> (),
	pause: (e: Ctx.Entry) -> (),
	resume: (e: Ctx.Entry) -> (),
	start: (e: Ctx.Entry) -> (),
	remove: (e: Ctx.Entry) -> (),
	request_changes: (e: Ctx.Entry, text: string) -> (),
	snooze: (ask: any, until_ms: number?) -> (),
	answer: (ask: any, choice: any) -> any,
	expand: (ask: any) -> (),
	menu: (e: Ctx.Entry) -> { any }?,
	snooze_menu: (ask: any) -> { any },
}

-- Why something was refused, in the engine's words where it gave any: a
-- promise's error (a table, or the host's `UiError`, by its `message`), or
-- what a call raised, its script position gone.
local function reason(err: any): string
	if type(err) == "string" then
		return (string.gsub(err, "^.-:%d+: ", ""))
	elseif type(err) == "table" or type(err) == "userdata" then
		local ok, m = pcall(function(): any
			return (err :: any).message or (err :: any).error
		end)
		if ok and type(m) == "string" then
			return m
		end
	end
	return if err ~= nil then tostring(err) else ""
end

-- Whether a card is closed (done or dropped).
local function closed(e: Ctx.Entry): boolean
	return e.place == "done" or e.place == "dropped"
end
M.closed = closed

-- The thread to open for a card: the one that asked, else its lead.
local function lead(e: Ctx.Entry): (string?, string?)
	local threads = e.item.threads or {}
	local a = e.ask
	if a and a.from_chat and table.find(threads, a.from_chat) then
		return a.from_chat, a.turn_id
	end
	return threads[1], nil
end
M.lead = lead

function M.make(ctx: Ctx.Ctx): Actions
	local props = ctx.props

	-- The host's prop called: its promise, or (a promise of) what it raised
	-- when the call itself failed (an app without the host action).
	local function call(f: (...any) -> any, ...: any): any
		local ok, sent = pcall(f, ...)
		if ok then
			return sent
		end
		return {
			after = function(self: any, _done: any, failed: ((any) -> ())?): any
				if failed then
					failed(sent)
				end
				return self
			end,
		}
	end

	-- `sent` said when it took (`done_key`, the card's title under it) or
	-- when it was refused (`failed_key`, with why). Said once per action: a
	-- move the engine makes after it is never said.
	local function said(sent: any, title: string, done_key: string?, failed_key: string)
		Ctx.after(sent, function(err: any)
			toast.show { title = locale.s(failed_key), body = reason(err), tone = "danger" }
		end, function()
			if done_key then
				toast.show { title = locale.s(done_key), body = title, tone = "success" }
			end
		end)
	end

	-- The title of the card an ask is on, if it's on one.
	local function title_of(ask: any): string?
		local e = ask and ask.item_id and ctx.entry(ask.item_id)
		return if e then e.item.title else nil
	end

	local act = {} :: Actions
	act.reason = reason
	function act.can(what: string): boolean
		return (props :: any)[`on_{what}`] ~= nil
	end
	function act.open_thread(e: Ctx.Entry, chat: string?)
		local open = props.on_open_thread
		if not open then
			return
		end
		if chat then
			open(chat)
			return
		end
		local to, message = lead(e)
		if to then
			open(to, message)
		end
	end
	function act.close(e: Ctx.Entry, dropped: boolean)
		local f = props.on_close
		if f then
			said(call(f, e.id, dropped), e.item.title, if dropped then "orch.board.said.dropped" else "orch.board.said.done", "orch.board.failed")
		end
	end
	function act.reopen(e: Ctx.Entry)
		local f = props.on_reopen
		if f then
			said(call(f, e.id), e.item.title, "orch.board.said.reopened", "orch.board.failed")
		end
	end
	function act.hold(e: Ctx.Entry, held: boolean)
		local f = props.on_hold
		if f then
			said(call(f, e.id, held), e.item.title, if held then "orch.board.said.held" else "orch.board.said.released", "orch.board.failed")
		end
	end
	function act.pause(e: Ctx.Entry)
		local f = props.on_pause
		if f then
			said(call(f, e.id), e.item.title, "orch.board.said.paused", "orch.board.pause_failed")
		end
	end
	function act.resume(e: Ctx.Entry)
		local f = props.on_resume
		if f then
			said(call(f, e.id, "active"), e.item.title, "orch.board.said.resumed", "orch.board.resume_failed")
		end
	end
	function act.start(e: Ctx.Entry)
		local f = props.on_start
		if f then
			said(call(f, e.id), e.item.title, "orch.board.said.started", "orch.board.start_failed")
		end
	end
	function act.request_changes(e: Ctx.Entry, text: string)
		local f = props.on_request_changes
		if f and text ~= "" then
			said(call(f, e.id, text), e.item.title, "orch.board.said.changes", "orch.board.failed")
		end
	end
	-- Taken off the board (its chat untouched): off it at once, and sent by
	-- the host once its Undo's while is over. Undo puts it back as it was:
	-- nothing was sent.
	function act.remove(e: Ctx.Entry)
		local f = props.on_remove
		if not f then
			return
		end
		local id, title = e.id, e.item.title
		local ok, sent = pcall(f, id, M.UNDO_MS)
		if not ok then
			toast.show { title = locale.s("orch.board.delete_failed"), body = reason(sent), tone = "danger" }
			return
		end
		Ctx.removing:set_key({ id }, true)
		local function back()
			Ctx.removing:set_key({ id }, nil)
		end
		if type(sent) == "table" or type(sent) == "userdata" then
			-- Sent: the engine's rows leave it out a moment later. Refused
			-- there: back, and why.
			sent:after(function()
				ui.after(M.LAND_MS, back)
			end, function(err: any)
				back()
				toast.show { title = locale.s("orch.board.delete_failed"), body = reason(err), tone = "danger" }
			end)
		else
			-- (A host that says nothing of how it went.)
			ui.after(M.UNDO_MS + M.LAND_MS, back)
		end
		local undo = props.on_undo_remove
		toast.show {
			title = locale.s("orch.board.deleted"),
			body = title,
			duration = M.UNDO_MS,
			action = if undo
				then {
					label = locale.s("orch.board.undo"),
					run = function()
						undo(id)
						back()
					end,
				}
				else nil,
		}
	end
	function act.snooze(ask: any, until_ms: number?)
		local f = props.on_snooze
		if not f or not ask then
			return
		end
		local title = title_of(ask)
		Ctx.after(call(f, ask.id, until_ms), function(err: any)
			toast.show { title = locale.s("orch.board.snooze_failed"), body = reason(err), tone = "danger" }
		end, function()
			toast.show { title = locale.s(if until_ms then "orch.board.said.snoozed" else "orch.board.said.back"), body = title, tone = "success" }
		end)
	end
	-- (Sent past its Undo by ds/orch_decide, which says a refusal on the
	-- ask itself, with Retry: only that it took is said here.)
	function act.answer(ask: any, choice: any): any
		local f = props.on_answer
		if not f then
			return nil
		end
		local sent = f(ask, choice)
		local title = title_of(ask)
		Ctx.after(sent, nil, function()
			toast.show { title = locale.s("orch.board.said.answered"), body = title, tone = "success" }
		end)
		return sent
	end
	function act.expand(ask: any)
		local f = props.on_expand_ask
		if f then
			f(ask)
		end
	end

	-- When to bring an ask back: in an hour, in four, tomorrow, next week.
	function act.snooze_menu(ask: any): { any }
		local entries: { any } = {}
		for _, t in decide.defer_times(ctx.at()) do
			table.insert(entries, {
				-- (No glyph each, as the ask overlay's Defer has none: the
				-- entry it hangs from has the clock.)
				label = decide.defer_label(t),
				on_select = function()
					act.snooze(ask, t.at)
				end,
			})
		end
		return entries
	end

	-- A card's menu, as it stands.
	function act.menu(e: Ctx.Entry): { any }?
		local entries: { any } = {}
		local threads = e.item.threads or {}
		if props.on_open_thread then
			for _, chat in threads do
				local t = ctx.thread(chat)
				table.insert(entries, {
					label = if #threads == 1 or not t
						then locale.s("orch.board.open_thread")
						else locale.s("orch.board.open_thread_named", { title = t.title or "" }),
					icon = "chat-round-line",
					on_select = function()
						act.open_thread(e, chat)
					end,
				})
			end
		end
		local is_closed = closed(e)
		if e.status == "paused" and props.on_resume then
			table.insert(entries, { label = locale.s("orch.board.resume"), icon = "action-play", on_select = function()
				act.resume(e)
			end })
		elseif e.place == "planned" and #threads == 0 and props.on_start then
			table.insert(entries, { label = locale.s("orch.board.start_now"), icon = "action-play", on_select = function()
				act.start(e)
			end })
		end
		-- (Pause stops what runs or waits on the person: a Planned card has
		-- Keep for later and Start now instead.)
		if not is_closed and e.place ~= "planned" and e.status ~= "paused" and props.on_pause then
			table.insert(entries, { label = locale.s("orch.board.pause"), icon = "action-pause", on_select = function()
				act.pause(e)
			end })
		end
		if e.place == "planned" and props.on_hold then
			if e.status == "held" then
				table.insert(entries, { label = locale.s("orch.board.release"), icon = "arrow-up", on_select = function()
					act.hold(e, false)
				end })
			elseif e.status == "next" or e.status == "queued" then
				table.insert(entries, { label = locale.s("orch.board.hold"), icon = "archive-minimalistic", on_select = function()
					act.hold(e, true)
				end })
			end
		end
		if e.ask and e.place == "needs_you" and props.on_snooze then
			table.insert(entries, { label = locale.s("orch.board.snooze"), icon = "clock-circle", submenu = act.snooze_menu(e.ask) })
		elseif e.place == "snoozed" and e.asks[1] and props.on_snooze then
			local ask = e.asks[1]
			table.insert(entries, { label = locale.s("orch.board.bring_back"), icon = "bell", on_select = function()
				act.snooze(ask, nil)
			end })
		end
		if #entries > 0 then
			table.insert(entries, { separator = true })
		end
		if is_closed then
			if props.on_reopen then
				table.insert(entries, { label = locale.s("orch.board.reopen"), icon = "restart", on_select = function()
					act.reopen(e)
				end })
			end
		elseif props.on_close then
			table.insert(entries, { label = locale.s("orch.board.mark_done"), icon = "check-circle", on_select = function()
				act.close(e, false)
			end })
			table.insert(entries, {
				label = locale.s("orch.board.drop"),
				icon = "close-circle",
				danger = true,
				confirm = locale.s("orch.board.drop_confirm"),
				on_select = function()
					act.close(e, true)
				end,
			})
		end
		local source = e.item.source
		if source and source.dump_id and props.on_source then
			table.insert(entries, { label = locale.s("orch.board.show_dump"), icon = "inbox", on_select = function()
				(props.on_source :: any)(source)
			end })
		end
		if props.on_remove then
			table.insert(entries, { separator = true })
			table.insert(entries, { label = locale.s("orch.board.delete"), icon = "trash-bin-minimalistic", danger = true, on_select = function()
				act.remove(e)
			end })
		end
		return if #entries > 0 then entries else nil
	end

	return act
end

return M

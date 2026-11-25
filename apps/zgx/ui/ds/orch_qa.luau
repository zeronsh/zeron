--!strict
-- The Control room's Q&A: each question the person dumped, with the answer
-- the thread it went to gave, waiting for them when they come back. An
-- entry is a card of the board's look (ds/orch_card): the question as its
-- title, under it a few lines of the answer, and one quiet line: where it
-- was answered
-- (a way into that thread), when it was asked and when answered, or that it
-- still waits. An answer not read yet is the one thing filled and marked:
-- a dot before it, its words in the primary ink; read ones step back.
-- Opening an entry (or its thread) reads it: a click opens it whole in the
-- decision overlay (ds/orch_decision's Answer: the answer as markdown, who
-- answered and when, a follow-up asked of the same thread); where the
-- pointer rests, it can be marked read or unread and put away.
--
-- The entries keep the order they were asked in (newest first) whatever
-- comes back, so nothing moves under the pointer or the scroll as answers
-- arrive.
--
--   local QA = require("@ds/orch_qa")
--   QA.marks        what was read and put away, on this device, kept
--                   across restarts (ui.persist `control_qa`):
--                   { read = { [key] = true }, dismissed = { [key] = true } }
--   QA.shown        the entry open in the overlay (its key), or nil:
--                   QA.show(entry) opens it (and reads it), QA.hide()
--   QA.ref(entry)   the entry as a reference (ds/orch_ref): what dragging
--                   it onto Capture carries, and what a follow-up sends
--                   with its words (the engine asks the thread that
--                   answered it)
--   QA.entries(dumps, marks, chats?) -> { Entry }      -- what it lists
--   QA.counts(entries) -> { unread, waiting }
--   QA.mark(marks, key, read), QA.dismiss(marks, key, away)
--   QA.Section {
--     entries = function() return QA.entries(...) end,
--     target = function(route) return data.Target end,  -- where it went
--     marks = QA.marks, open = QA.opened,
--     on_open_thread = function(chat_id) end,
--   }
--
-- The whole of an entry's model and its state is here: the page only
-- feeds it the dumps and opens threads.

local FoldScroll = require("@self/fold_scroll")
local IconButton = require("@self/icon_button").IconButton
local ContextMenu = require("@self/context_menu")
local Icon = require("@self/icon")
local status = require("@self/status")
local locale = require("@self/locale")
local hue = require("@self/orch_hue")
local data = require("@self/orch_data")
local Card = require("@self/orch_card")
local toast = require("@self/toast")
local Ref = require("@self/orch_ref")

local M = {}

export type Status = "waiting" | "unread" | "read"

export type Entry = {
	key: string,
	dump_id: string,
	route_id: string,
	question: string,
	-- The whole reply (markdown), or the one line when that's all there is.
	answer: string?,
	route: any,
	-- The chat it went to (where it is answered), when there is one.
	chat: string?,
	asked: number,
	answered: number?,
	status: Status,
	-- Its thread is at work on it now.
	answering: boolean,
}

export type Marks = { read: { [string]: boolean }?, dismissed: { [string]: boolean }? }

M.marks = ui.persist("control_qa", { read = {}, dismissed = {} } :: Marks)
M.opened = ui.state("control_qa_open", {} :: { [string]: boolean })
M.shown = ui.state("control_qa_shown", nil :: string?)
M.all = ui.state("control_qa_all", false)

-- An entry put away, with a moment to take it back (Undo).
function M.put_away(entry: any, marks: any?)
	local m = marks or M.marks
	local k = entry.key
	M.dismiss(m, k, true)
	toast.show {
		key = `orch-qa-dismissed-{k}`,
		title = locale.s("ds.orch.qa.dismissed"),
		body = entry.question,
		duration = 6000,
		action = {
			label = locale.s("ds.orch.qa.undo"),
			run = function()
				M.dismiss(m, k, false)
			end,
		},
	}
end

-- The entry open in the overlay: shown whole, its answer read.
function M.show(entry: any)
	if entry.answer ~= nil and entry.status == "unread" then
		M.mark(M.marks, entry.key, true)
	end
	M.shown:set(entry.key)
end

function M.hide()
	M.shown:set(nil)
end

-- The entry shown, from `entries` (nil: none, or not among them).
function M.shown_of(entries: { any }): any
	local key = M.shown:get()
	if not key then
		return nil
	end
	for _, e in entries do
		if e.key == key then
			return e
		end
	end
	return nil
end

-- An entry as a reference (ds/orch_ref): its route, its question, the dump
-- it was asked in and the chat that answers it.
function M.ref(en: any): any
	return Ref.make("qa", en.route_id, en.question, { dump = en.dump_id, chat = en.chat, answer = en.answer })
end

-- An entry's key: its dump's and route's ids.
function M.key(dump_id: string, route_id: string): string
	return `{dump_id}/{route_id}`
end

local function text_of(v: any): string?
	return if type(v) == "string" and v ~= "" then v else nil
end

-- The chat a route went to (as ds/orch_receipt's "way in").
local CHAT_KINDS: { [string]: boolean } = { chat = true, orchestrator = true, new_thread = true, new_orchestrator = true }
local function chat_of(target: any): string?
	if target and CHAT_KINDS[target.kind] and text_of(target.id) then
		return target.id
	end
	return nil
end

-- Whether a route is one of the person's questions: the host marks it
-- (`direct`: tagged a question by the Brain dump and, in the person's own
-- words, neither work nor rhetoric); a route from a host before that, by
-- its tag alone.
function M.is_question(route: any): boolean
	if route.direct ~= nil then
		return route.direct == true
	end
	return route.intent == "question"
end

-- Every question in `dumps` (newest dump first, each dump's in the order
-- said), but those put away or moved off: how each stands, by `marks`.
function M.entries(dumps: { any }, marks: Marks?, chats: { [string]: any }?): { Entry }
	local read = (marks and marks.read) or {}
	local dismissed = (marks and marks.dismissed) or {}
	local out: { Entry } = {}
	for _, d in dumps or {} do
		for _, r in d.routes or {} do
			local key = M.key(d.id, r.id)
			if M.is_question(r) and r.state ~= "undone" and not dismissed[key] then
				local answer = text_of(r.answer_full) or text_of(r.answer)
				local answered = answer ~= nil and (r.state == "answered" or r.state == "done" or r.state == "failed")
				local chat = chat_of(r.target)
				local c = chat and chats and chats[chat]
				table.insert(out, {
					key = key,
					dump_id = d.id,
					route_id = r.id,
					question = r.excerpt or "",
					answer = if answered then answer else nil,
					route = r,
					chat = chat,
					asked = d.created or 0,
					answered = if answered then (r.answered or d.updated) else nil,
					status = (if not answered then "waiting" elseif read[key] then "read" else "unread") :: Status,
					answering = not answered and M.at_it(c, r.delivered or d.created),
				})
			end
		end
	end
	return out
end

-- Whether chat `c` is at a question delivered at `delivered`: working,
-- on a turn begun since (a turn that was already going when it came, the
-- thread busy with something else, hasn't reached it yet).
function M.at_it(c: any, delivered: number?): boolean
	if not c or c.status ~= "working" or type(c.turn_started) ~= "number" then
		return false
	end
	return delivered == nil or c.turn_started >= delivered - 1
end

-- How many wait to be read, and how many wait on an answer.
function M.counts(entries: { Entry }): { unread: number, waiting: number }
	local unread, waiting = 0, 0
	for _, e in entries do
		if e.status == "unread" then
			unread += 1
		elseif e.status == "waiting" then
			waiting += 1
		end
	end
	return { unread = unread, waiting = waiting }
end

-- An answer's words as one quiet run (its marks left out), for the
-- preview.
function M.plain(md: string): string
	local s = string.gsub(md, "```[^`]*```", " ")
	s = string.gsub(s, "%*%*", "")
	s = string.gsub(s, "__", "")
	s = string.gsub(s, "`", "")
	s = string.gsub(s, "%[([^%]]*)%]%b()", "%1")
	s = string.gsub(s, "\n%s*[-*+]%s+", "\n")
	s = string.gsub(s, "\n#+%s*", "\n")
	s = string.gsub(s, "^#+%s*", "")
	s = string.gsub(s, "%s*\n+%s*", " ")
	return (string.gsub(s, "^%s+", ""))
end

-- Marked read (true) or unread (false).
function M.mark(marks: any, key: string, read: boolean)
	local m = marks:peek() or {}
	local r = table.clone(m.read or {})
	r[key] = if read then true else nil
	marks:set({ read = r, dismissed = table.clone(m.dismissed or {}) })
end

-- Put away (true) or back (false).
function M.dismiss(marks: any, key: string, away: boolean)
	local m = marks:peek() or {}
	local d = table.clone(m.dismissed or {})
	d[key] = if away then true else nil
	marks:set({ read = table.clone(m.read or {}), dismissed = d })
end

export type Props = {
	entries: () -> { Entry },
	target: (route: any) -> any,
	marks: any,
	open: any,
	on_open_thread: (chat: string) -> (),
	-- How many show before "N more" (all once that's opened).
	limit: number?,
	-- Whether every entry shows (a state; its own when not given).
	all: any,
}

-- How many lines of an answer the preview shows: more while unread.
local PREVIEW_UNREAD, PREVIEW_READ = 3, 1

local function entry(props: Props, row: State<Entry>): Node
	local hovered = ui.use_state(false)
	local menu_open = ui.use_state(false)
	local function e(): Entry
		return row:get()
	end
	local function key(): string
		return row:peek().key
	end
	local function answered(): boolean
		return row:get().answer ~= nil
	end
	local function unread(): boolean
		return row:get().status == "unread"
	end
	local function target(): data.Target
		return props.target(row:get().route)
	end
	local function read(on: boolean)
		M.mark(props.marks, key(), on)
	end
	local function open_thread()
		local c = row:peek().chat
		if not c then
			return
		end
		if answered() then
			read(true)
		end
		props.on_open_thread(c)
	end
	local function dismiss()
		M.put_away(row:peek(), props.marks)
	end
	local function items(): { any }
		local out: { any } = {}
		if row:peek().chat then
			table.insert(out, { label = locale.s("ds.orch.qa.open_thread"), icon = "arrow-up-right", on_select = open_thread })
		end
		if answered() then
			if unread() then
				table.insert(out, { label = locale.s("ds.orch.qa.mark_read"), icon = "eye", on_select = function()
					read(true)
				end })
			else
				table.insert(out, { label = locale.s("ds.orch.qa.mark_unread"), icon = "eye-closed", on_select = function()
					read(false)
				end })
			end
		end
		table.insert(out, { separator = true })
		table.insert(out, { label = locale.s("ds.orch.qa.dismiss"), icon = "close", on_select = dismiss })
		return out
	end

	-- Before the title: the unread mark, in a gutter every entry keeps so
	-- their words line up.
	local gutter = ui.div {
		class = "orch-qa-gutter",
		w = 6,
		h = (ui.theme.get("font.line.sm") :: number?) or 18,
		shrink = 0,
		items = "center",
		justify = "center",
		ui.show(unread, ui.div {
			class = "orch-qa-unread",
			role = "img",
			label = locale.t("ds.orch.qa.unread"),
			w = 6,
			h = 6,
			rounded = "$radius.full",
			bg = "$accent",
		}),
	}

	-- Its actions, where the pointer rests: read or unread, and away.
	local actions = ui.show(function(): boolean
		return hovered:get() or menu_open:get()
	end, ui.row {
		class = "orch-qa-actions",
		shrink = 0,
		gap = "$zeron.space.hair",
		items = "center",
		ui.show(answered, IconButton({
			class = "orch-qa-read",
			icon = function(): Icon.Name
				return if unread() then "eye" else "eye-closed"
			end,
			label = function(): string
				return locale.s(if unread() then "ds.orch.qa.mark_read" else "ds.orch.qa.mark_unread")
			end,
			size = "sm",
			tone = "quiet",
			on_click = function()
				read(unread())
			end,
		} :: any)),
		IconButton {
			class = "orch-qa-dismiss",
			icon = "close",
			label = locale.t("ds.orch.qa.dismiss"),
			size = "sm",
			tone = "quiet",
			on_click = dismiss,
		},
	})

	local title = ui.row {
		w_full = true,
		min_w = 0,
		gap = "$space.2",
		items = "start",
		gutter,
		ui.column {
			grow = 1,
			min_w = 0,
			Card.Title {
				text = function(): string
					return (string.gsub(e().question, "%s+", " "))
				end,
				quiet = function(): boolean
					return e().status == "read"
				end,
			},
		},
		actions,
	}

	-- The answer: a few lines of it (all of it in the overlay).
	local answer = ui.show(answered, ui.presence(function(): string
		return if unread() then "unread" else "read"
	end, function(kind: string): Node
		return ui.text {
			function(): string
				return M.plain(e().answer or "")
			end,
			class = "orch-qa-preview",
			w_full = true,
			min_w = 0,
			pl = ui.raw(14, "past the gutter, under the question's words"),
			wrap = true,
			lines = if kind == "unread" then PREVIEW_UNREAD else PREVIEW_READ,
			truncate = true,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = if kind == "unread" then "$text.secondary" else "$text.tertiary",
		}
	end, { w_full = true, min_w = 0 }))

	-- The quiet line: where it was answered, when asked, when answered (or
	-- that it waits), and whether it opens further.
	local function where(): string
		return target().label
	end
	local line = Card.Line {
		class = "orch-qa-status",
		-- (Under the question's words: the gutter and its gap, less the
		-- line's own gap.)
		ui.div { w = ui.raw(10, "the unread gutter and its gap, less the line's gap"), shrink = 0 },
		ui.show(function(): boolean
			return row:get().chat ~= nil
		end, ui.button {
			class = "orch-qa-thread",
			label = function(): string
				return locale.s("ds.orch.qa.open_in", { name = where() })
			end,
			min_w = 0,
			shrink = 1,
			h = "auto",
			p = 0,
			gap = "$space.1",
			items = "center",
			bg = "#00000000",
			hover = { bg = "#00000000" },
			on_click = open_thread,
			hue.Dot {
				hue = function(): number?
					return target().hue
				end,
				size = 6,
			},
			ui.text {
				where,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.xs",
				color = "$text.secondary",
			},
		}),
		Card.Sep(function(): boolean
			return row:get().chat ~= nil
		end),
		Card.Word {
			class = "orch-qa-asked",
			text = function(): string
				return locale.s("ds.orch.qa.asked", { ago = status.ago(status.since(e().asked)) })
			end,
			shrink = 0,
		},
		Card.Sep(function(): boolean
			return true
		end),
		Card.Word {
			class = "orch-qa-when",
			text = function(): string
				local x = e()
				if x.answered then
					return locale.s("ds.orch.qa.answered", { ago = status.ago(status.since(x.answered)) })
				elseif x.answer then
					return locale.s("ds.orch.qa.answered_now")
				end
				return locale.s(if x.answering then "ds.orch.qa.answering" else "ds.orch.qa.waiting")
			end,
			shrink = 0,
			strong = false,
			ink = function(): string
				local x = e()
				return if x.answer == nil and x.answering then "$zeron.status.working" else "$text.tertiary"
			end,
		},
		ui.div { grow = 1 },
	}

	-- Dragged: a reference to the question (ds/orch_ref), for Capture.
	local drag = Ref.source(function(): any
		return M.ref(row:peek())
	end)
	local box = ui.button {
		class = "orch-qa-entry",
		on_drag = drag.on_drag,
		drag_preview = drag.drag_preview,
		drag_preview_anchor = drag.drag_preview_anchor,
		drag_preview_offset = drag.drag_preview_offset,
		label = function(): string
			return e().question
		end,
		w_full = true,
		p = 0,
		items = "start",
		rounded = Card.RADIUS,
		-- Filled only while its answer waits to be read (and under the
		-- pointer): the rest sit on the page.
		bg = (function(): () -> string
			local fill = Card.bg(function(): boolean
				return hovered:get()
			end)
			return function(): string
				if hovered:get() then
					return "$hover"
				end
				return if unread() then fill() else "#00000000"
			end
		end)(),
		hover = {},
		active = { bg = "$pressed" },
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		-- Opened whole in the overlay (and read).
		on_click = function()
			M.show(row:peek())
		end,
		ui.column {
			w_full = true,
			min_w = 0,
			p = Card.pad(false),
			gap = "$zeron.space.snug",
			title,
			answer,
			line,
		},
	}
	return ContextMenu {
		items = items,
		open = menu_open,
		label = locale.t("ds.orch.qa.actions"),
		ring = Card.RADIUS,
		w_full = true,
		box,
	}
end

-- How tall the list stands once "n more" opened it, before it scrolls.
local OPEN_MAX_H = 720

local Section = ui.component(function(props: Props): Node
	local own_all = ui.use_state(false :: boolean)
	local all: any = if props.all ~= nil then props.all else own_all
	local limit: number = if props.limit then props.limit else 6
	local list = ui.computed(function(): { Entry }
		return props.entries()
	end)
	local shown = ui.computed(function(): { Entry }
		local es = list:get()
		if all:get() or #es <= limit then
			return es
		end
		local out = {}
		for i = 1, limit do
			out[i] = es[i]
		end
		return out
	end)
	return ui.column {
		id = "orch-qa",
		class = "orch-qa",
		w_full = true,
		gap = "$space.1",
		ui.show(function(): boolean
			return #list:get() == 0
		end, ui.text {
			locale.t("ds.orch.qa.empty"),
			class = "orch-qa-empty",
			text_size = "$font.size.sm",
			color = "$text.tertiary",
			py = "$space.1",
		}),
		-- (Keyed: an entry is built once and follows its route as it changes,
		-- so an answer arriving never builds the list again.)
		-- (Opened past `limit`: about ten entries tall, then it scrolls.)
		FoldScroll {
			class = "orch-qa-list",
			open = function(): boolean
				return all:get()
			end,
			max_h = OPEN_MAX_H,
			list = function(): Node
				return ui.each(shown, function(_e: Entry, _i: number, row: State<Entry>): Node
					return entry(props, row)
				end, { key = function(x: Entry): string
					return x.key
				end, w_full = true, gap = "$zeron.space.hair" })
			end,
		},
		ui.show(function(): boolean
			return #list:get() > limit
		end, ui.button {
			class = "orch-qa-more",
			label = function(): string
				local n = #list:get() - limit
				return if all:get() then locale.s("ds.orch.qa.fewer") else locale.s("ds.orch.qa.more", { n = n })
			end,
			expanded = function(): boolean
				return all:get()
			end,
			self = "start",
			h = 28,
			px = ui.raw(10, "the cards' inset: its words under theirs"),
			rounded = "$radius.md",
			bg = "#00000000",
			hover = { bg = "$hover" },
			items = "center",
			on_click = function()
				all:set(not all:peek())
			end,
			ui.text {
				function(): string
					local n = #list:get() - limit
					return if all:get() then locale.s("ds.orch.qa.fewer") else locale.s("ds.orch.qa.more", { n = n })
				end,
				text_size = "$font.size.sm",
				color = "$text.secondary",
			},
		}),
	}
end)

M.Section = Section

return M

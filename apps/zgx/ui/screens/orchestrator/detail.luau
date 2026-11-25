-- What opens in an orchestrator's side panel in place of its tabs. A card:
-- the board's own detail of it (ds/orch_board_detail's Card, with Back).
-- A crew thread: a back control, its title and how it stands, the ask it
-- waits on with its answers, its last report, the machine and model it
-- runs on, its progress, and "Open thread" (the only way from here to the
-- thread itself).
--
--   Detail { of = function() return { chat = id, item = id? } end,
--     board = board props (screens/orchestrator/tasks' board_props),
--     orchestrator = function() return id end, name_of = fn, device_name = fn,
--     on_back = fn, on_open_thread = fn(chat) }

local ds = require("@ds")
local locale = require("@ds/locale")
local status = require("@ds/status")
local Name = require("@ds/orch_name")
local Asks = require("@ds/orch_asks")
local CrewList = require("@ds/orch_crew_list")
local Mark = require("@ds/mark")
local decide = require("@ds/orch_decide")
local orchestration = require("@account/orchestration")
local sessions = require("@account/sessions")
local NeedsYou = require("@app/screens/control/needs")
local data = require("@ds/orch_data")
local Progress = require("@app/screens/control/progress")
local BoardDetail = require("@ds/orch_board_detail")

local M = {}

local UNDER_WAY: { [string]: boolean } = { working = true, connecting = true, waiting = true }
local STATES: { [string]: boolean } = { planned = true, active = true, review = true, blocked = true, done = true }

export type Props = {
	of: () -> { chat: string?, item: string? }?,
	-- The board's props (ds/orch_board_ctx): a card's detail is the
	-- board's own (ds/orch_board_detail's Card), not this one.
	board: { [string]: any }?,
	orchestrator: () -> string?,
	name_of: (id: string?) -> string?,
	device_name: (id: string) -> string?,
	on_back: () -> (),
	on_open_thread: (chat: string) -> (),
}

-- A field: its icon, its words.
local function field(id: string, icon: Node, text: () -> string): Node
	return ui.row {
		id = id,
		gap = "$space.2",
		items = "center",
		min_w = 0,
		hidden = function(): boolean
			return text() == ""
		end,
		ui.div { color = "$text.tertiary", shrink = 0, w = 16, items = "center", justify = "center", icon },
		ui.text { text, min_w = 0, truncate = true, text_size = "$font.size.sm", color = "$text.secondary" },
	}
end

local function heading(key: string): Node
	return ui.text {
		locale.t(key),
		text_size = "$font.size.xs",
		weight = "$font.weight.medium",
		color = "$text.tertiary",
	}
end

-- A card opened: the board's detail of it, with Back.
local function card_detail(props: Props): Node
	local board: { [string]: any } = table.clone(props.board or {})
	-- (Back leaves the card as it is; the board's `on_close` stays Mark
	-- done and Drop.)
	board.on_back = props.on_back
	board.card = function(): string?
		local d = props.of()
		return d and d.item
	end
	board.back = true
	board.flat = true
	return ui.scroll {
		id = "orch-detail",
		w_full = true,
		grow = 1,
		min_h = 0,
		fade = { 12, 12 },
		fade_overflow = true,
		ui.column {
			w_full = true,
			px = "$space.3",
			py = "$space.2",
			BoardDetail.Card(board :: any),
		},
	}
end

function M.Detail(props: Props): Node
	return ui.presence(function(): string
		local d = props.of()
		return if d and d.item and props.board then "card" else "thread"
	end, function(kind: string): Node
		if kind == "card" then
			return card_detail(props)
		end
		return M.Thread(props)
	end, { w_full = true, grow = 1, min_h = 0 })
end

-- A crew thread opened: how it stands, its ask, its last report, where it
-- runs, and Open thread.
function M.Thread(props: Props): Node
	local function chat_id(): string?
		local d = props.of()
		return d and d.chat
	end
	local function item(): any
		local d = props.of()
		local id = d and d.item
		if not id then
			return nil
		end
		for _, it in (orchestration.now:get() or {}).items or {} do
			if it.id == id then
				return it
			end
		end
		return nil
	end
	local function chat(): any
		local id = chat_id()
		if not id then
			return nil
		end
		return data.chats(sessions.now:get())[id]
	end
	local function member(): any
		local id = chat_id()
		for _, m in (orchestration.now:get() or {}).crew or {} do
			if m.chat_id == id then
				return m
			end
		end
		return nil
	end
	local function title(): string
		local it = item()
		if it and it.title and it.title ~= "" then
			return it.title
		end
		local id = chat_id()
		return Name.crew(chat(), Name.fallback(member(), nil), id)
	end
	-- The ask of its that's open (its thread's or its card's), if one is.
	local function open_ask(): any
		local id, it = chat_id(), item()
		for _, a in (orchestration.now:get() or {}).asks or {} do
			if (a.state == nil or a.state == "open") and ((id and a.from_chat == id) or (it and a.item_id == it.id)) then
				return a
			end
		end
		return nil
	end
	-- How it stands: waiting on you while an ask of its is open; else a
	-- card's column, else its thread's state. (A card's state drawn as a
	-- thread's would be.)
	local ITEM_KIND: { [string]: string } = { planned = "idle", active = "working", review = "approval", blocked = "limited", done = "done" }
	local function state_kind(): string
		local a = open_ask()
		if a then
			return if a.kind == "approval" then "approval" else "input"
		end
		local it = item()
		if it and ITEM_KIND[it.state or ""] then
			return ITEM_KIND[it.state]
		end
		local c = chat()
		local st = (c and c.status) or ""
		if UNDER_WAY[st] then
			return "working"
		elseif status.KINDS[st :: any] then
			return st
		end
		return "idle"
	end
	local function state_word(): string
		if open_ask() then
			return locale.s("ds.orch.crew_list.waiting")
		end
		local it = item()
		if it and STATES[it.state or ""] then
			return locale.s(`orch.panel.state.{it.state}`)
		end
		local k = state_kind()
		if k == "idle" then
			return locale.s("ds.orch.crew_list.idle_word")
		end
		local info = status.KINDS[k :: any]
		return if info then info.long else ""
	end
	-- What waits on the person from it: its ask (its thread's or its card's),
	-- or its thread awaiting input.
	local function needs(): { any }
		local id, it = chat_id(), item()
		local out = {}
		for _, n in NeedsYou.of(orchestration.now:get(), sessions.now:get(), props.orchestrator()) do
			local a = n.ask
			if n.kind == "card" then
				-- (A card's row: its card, or one of its asks this thread's.)
				local mine = (it and n.card == it.id) or (id and n.from == id)
				for _, x in n.asks or {} do
					mine = mine or (id ~= nil and x.from_chat == id)
				end
				if mine then
					table.insert(out, n)
				end
			elseif a then
				if (id and a.from_chat == id) or (it and a.item_id == it.id) then
					table.insert(out, n)
				end
			elseif id and n.from == id then
				table.insert(out, n)
			end
		end
		return out
	end
	local function report(): any
		local m = member()
		return m and m.last_report
	end

	local top = ui.row {
		w_full = true,
		gap = "$space.2",
		items = "center",
		ds.IconButton {
			id = "orch-thread-back",
			icon = "arrow-left",
			label = ui.t("orch.panel.back") :: any,
			size = "sm",
			tone = "quiet",
			on_click = props.on_back,
		},
		ui.text {
			title,
			id = "orch-thread-title",
			grow = 1,
			min_w = 0,
			truncate = true,
			text_size = "$font.size.md",
			weight = "$font.weight.semibold",
			color = "$text.primary",
		},
	}

	local state = ui.row {
		id = "orch-thread-status",
		gap = "$space.2",
		items = "center",
		ui.div {
			w = 16,
			items = "center",
			justify = "center",
			ui.presence(state_kind, function(kind: string): Node
				-- (Its kind as a class: what tests read.)
				if kind == "idle" then
					return ui.div { class = "orch-status-idle", color = "$text.tertiary", ds.Icon { name = "clock-circle", size = 14 } }
				end
				return ui.div { class = `orch-status-{kind}`, status.StatusIcon { kind = kind :: any, size = 14 } }
			end),
		},
		ui.text { state_word, text_size = "$font.size.sm", color = "$text.primary" },
		ui.text {
			function(): string
				-- (Waiting: since it asked.)
				local a = open_ask()
				local c = chat()
				local it = item()
				local t = (a and type(a.created) == "number" and a.created) or (c and c.updated) or (it and it.updated)
				return if t then status.ago(math.max(0, os.time() - t)) else ""
			end,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
			tabular = true,
		},
	}

	local ask = ui.show(function(): boolean
		return #needs() > 0
	end, ui.column {
		id = "orch-thread-ask",
		w_full = true,
		gap = "$space.1",
		heading("orch.panel.detail.waiting"),
		Asks {
			needs = needs,
			recent = function(): { any }
				return {}
			end,
			name_of = props.name_of,
			on_answer = NeedsYou.answer,
			on_dismiss = function(a: any)
				orchestration.dismiss(a.id)
			end,
			on_open = function(c: string)
				props.on_open_thread(c)
			end,
			on_expand = function(a: any)
				decide.show(a.id)
			end,
			limit = 3,
		},
	})

	local said = ui.show(function(): boolean
		return report() ~= nil
	end, ui.column {
		id = "orch-thread-report",
		w_full = true,
		gap = "$space.1",
		heading("orch.panel.detail.report"),
		ui.text {
			function(): string
				local r = report()
				return (r and r.summary) or ""
			end,
			w_full = true,
			wrap = true,
			text_size = "$font.size.sm",
			color = "$text.primary",
		},
		ui.text {
			function(): string
				local r = report()
				return if r and type(r.at) == "number" then status.ago(math.max(0, os.time() - r.at)) else ""
			end,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		},
	})

	-- What it's doing now (its activity, the question it asked), as the
	-- Crew tab says it.
	local function doing(): string
		local m = member()
		-- (Its ask, shown above with its answers, isn't said twice.)
		if not m or #needs() > 0 then
			return ""
		end
		local row = { member = m, chat = chat() }
		return CrewList.doing(row, CrewList.asked_by((orchestration.now:get() or {}).asks))
	end
	local where = ui.column {
		w_full = true,
		gap = "$space.2",
		field("orch-thread-doing", ds.Icon { name = "chat-round-line", size = 14 }, doing),
		field("orch-thread-machine", ds.Icon { name = "monitor", size = 14 }, function(): string
			local c = chat()
			return (c and c.device and props.device_name(c.device)) or ""
		end),
		field(
			"orch-thread-model",
			ui.presence(function(): string
				local c = chat()
				return (c and c.agent) or ""
			end, function(agent: string): Node
				return if agent ~= "" then Mark { name = agent, size = 14, bare = true } else ds.Icon { name = "cpu", size = 14 }
			end),
			function(): string
				local c = chat()
				return CrewList.model_name(c and c.model) or ""
			end
		),
	}

	return ui.scroll {
		id = "orch-detail",
		w_full = true,
		grow = 1,
		min_h = 0,
		fade = { 12, 12 },
		fade_overflow = true,
		ui.column {
			w_full = true,
			gap = "$space.4",
			px = "$space.3",
			py = "$space.2",
			top,
			state,
			ask,
			said,
			where,
			-- Its progress: clocks and menu (pinned to Progress from there).
			ui.presence(function(): string
				return chat_id() or ""
			end, function(id: string): Node
				if id == "" then
					return ui.div {}
				end
				return ui.row {
					w_full = true,
					gap = "$space.2",
					items = "center",
					hidden = function(): boolean
						return Progress.row(id) == nil
					end,
					heading("orch.panel.detail.progress"),
					ui.div { grow = 1, min_w = 0, Progress.strip(id, { lean = true }) },
				}
			end, { w_full = true, id = "orch-thread-progress" }),
			ui.show(function(): boolean
				return chat_id() ~= nil
			end, ui.div {
				ds.Button {
					ui.t("orch.panel.open_thread") :: any,
					id = "orch-thread-open",
					self = "start",
					tone = "secondary",
					size = "sm",
					on_click = function()
						local id = chat_id()
						if id then
							props.on_open_thread(id)
						end
					end,
				},
			}),
		},
	}
end

return M

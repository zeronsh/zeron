--!strict
-- What the engine says into an orchestrator's (or a crew thread's, or the
-- Brain dump's) conversation, as its transcript shows it: a quiet block in
-- Crew updates' look (ds/orch_reports), never the person's bubble. The
-- agent is sent the whole text, unchanged; what's only for the agent (the
-- ⟦crew-output⟧ fences, `answer_ask`'s instructions, the routing steps)
-- never shows.
--
-- Which kind a message is comes from the engine (`injected`, src/chat.rs
-- from the entry's `injected` part); a message written before it carried
-- one is read from its text (`parse`).
--
--   local Injected = require("@self/orch_injected")
--   Injected.parse(text)        -- { kind, … } for a known header, else nil
--   Injected.read(text, injected)  -- the engine's kind and ids, its words
--                               -- read from its text (or all of it, without)
--   Injected.known(i)           -- i, if this build draws its kind
--   Injected.bubble(i)          -- the person's (or a sender's) words stay a
--                               -- bubble: { body, rules?, quoted? } or nil
--   Injected.Block {
--     injected = i, at = 1731800000,   -- unix seconds
--     title_of = function(chat) return title end,
--     ask_of = function(id) return ask end,         -- orchestration's asks
--     open_ask = function(id) end,                  -- the decision overlay
--     open_chat = function(chat, message?) end,     -- a thread, at a message
--   }
--
-- Kinds (`kind`, and what each carries):
--   crew_ask        ask, thread (its title then), question, options
--   answer          by ("you" | "orchestrator"), question, answer
--   for_thread      chat, thread, message       (a relay to a crew thread)
--   on_card         card, message
--   changes         card, text                  (Review's Request changes)
--   retraction      excerpt                     (a route moved elsewhere)
--   board_check     cards = { { title, id, note } }
--   resumed         card
--   scheduled       prompt?                     (a scheduled wake)
--   dump            dump, body                  (the person's words: a bubble)
--   crew_rules      orchestrator, rules, body   (a bubble, the rules folded)
--   quoted          from, chat, body            (a bubble)

local Icon = require("@self/icon")
local Badge = require("@self/badge")
local status = require("@self/status")
local locale = require("@self/locale")

local M = {}

export type Injected = { [string]: any }
export type Props = {
	injected: Injected,
	at: number?,
	title_of: ((chat: string) -> string?)?,
	ask_of: ((id: string) -> any?)?,
	-- An answer an older engine sent names no ask: the ask its question is.
	ask_by_question: ((question: string) -> any?)?,
	open_ask: ((id: string) -> ())?,
	open_chat: ((chat: string, message: string?) -> ())?,
}

local OPEN = "\u{27E6}crew-output\u{27E7}"
local CLOSE = "\u{27E6}/crew-output\u{27E7}"

local function trim(s: string): string
	return (string.gsub(s, "^%s+", ""):gsub("%s+$", ""))
end

-- The text inside the first fenced block in `s` (its lines' indent gone),
-- and what follows it.
local function fenced(s: string): (string?, string)
	local a, b = string.find(s, OPEN, 1, true)
	if not a then
		return nil, s
	end
	local c, d = string.find(s, CLOSE, (b :: number) + 1, true)
	if not c then
		return nil, s
	end
	local inner = string.sub(s, (b :: number) + 1, (c :: number) - 1)
	local lines: { string } = {}
	for line in string.gmatch(inner, "[^\n]+") do
		table.insert(lines, trim(line))
	end
	return trim(table.concat(lines, "\n")), string.sub(s, (d :: number) + 1)
end

-- A message's text, as the engine wrote it before it said which kind:
-- the known headers (crates/proto orchestration `prompt_text`,
-- `role_prompts`, the engine's own). Nil for anything else.
function M.parse(text: string?): Injected?
	if type(text) ~= "string" then
		return nil
	end
	local t = text
	if string.sub(t, 1, 1) ~= "[" then
		local default = "Scheduled check-in: look over your board and crew, and move what's stuck."
		if trim(t) == default then
			return { kind = "scheduled", legacy = true }
		end
		return nil
	end
	-- [Crew ask] <thread> ended its turn waiting on a decision; … ⟦crew-output⟧ … (id <ask>) …
	local thread = string.match(t, "^%[Crew ask%] (.-) ended its turn waiting on a decision;")
	if thread then
		local after = string.match(t, "\n(.*)$") or ""
		local question, rest = fenced(after)
		local options: { string } = {}
		local opts = fenced(rest)
		if opts then
			local list = string.match(opts, "^Options: (.*)$")
			if list then
				for o in string.gmatch(list .. " | ", "(.-) | ") do
					table.insert(options, o)
				end
			end
		end
		local ask = string.match(t, "`answer_ask` %(id ([^%)]+)%)")
		return { kind = "crew_ask", legacy = true, thread = thread, question = question or "", options = options, ask = ask }
	end
	local by, qa = string.match(t, "^%[Answer from (y[%a ]-)%] (.*)$")
	if by then
		local question, answer = string.match(qa :: string, "^(.-) \u{2192} (.*)$")
		return {
			kind = "answer",
			legacy = true,
			by = if by == "your orchestrator" then "orchestrator" else "you",
			question = question or qa,
			answer = answer or "",
		}
	end
	local name, rules, body = string.match(t, "^%[Crew rules from ([^%]\n]-)%]\n(.-)\n\n(.*)$")
	if name then
		return { kind = "crew_rules", legacy = true, orchestrator = name, rules = rules, body = body }
	end
	local ft_title, ft_chat, ft_message = string.match(t, "^%[For your crew thread (.-) %(([^%s%(%)]+)%)%] (.*)$")
	if ft_title then
		return { kind = "for_thread", legacy = true, thread = ft_title, chat = ft_chat, message = ft_message }
	end
	local card, message = string.match(t, '^%[On your card "(.-)"%] (.*)$')
	if card then
		return { kind = "on_card", legacy = true, card = card, message = message }
	end
	local ch_card, ch_text = string.match(t, '^%[Changes requested on "(.-)"%] (.*)$')
	if ch_card then
		return { kind = "changes", legacy = true, card = ch_card, text = ch_text }
	end
	local excerpt = string.match(t, '^%[Brain dump%] Ignore the earlier note "(.-)": it was meant elsewhere%.')
	if excerpt then
		return { kind = "retraction", legacy = true, excerpt = excerpt }
	end
	local dump, dump_body = string.match(t, "^%[Dump ([%w%-_]+)%] Route this; don't act on it%..-\n\n(.*)$")
	if dump then
		-- (The elements dragged in, listed for the agent: not the words.)
		local words = string.match(dump_body :: string, "^(.-)\n\nReferences the person attached") or dump_body
		return { kind = "dump", legacy = true, dump = dump, body = words }
	end
	local checks = string.match(t, "^%[Board check%][^\n]*\n(.*)$")
	if checks then
		local cards: { any } = {}
		for line in string.gmatch(checks, "[^\n]+") do
			local title, id, note = string.match(line, '^%- "(.-)" %(([^%)]+)%): (.*)$')
			if title then
				table.insert(cards, { title = title, id = id, note = note })
			end
		end
		return { kind = "board_check", legacy = true, cards = cards }
	end
	local resumed = string.match(t, '^%[Resumed%] The person resumed "(.-)": carry on where you stopped%.')
	if resumed then
		return { kind = "resumed", legacy = true, card = resumed }
	end
	-- The engine back after a restart: carry on, or (its turn ended into a
	-- wait on background work the restart stopped) run that again.
	if string.find(t, "^%[Resumed%] The engine restarted") then
		return { kind = "resumed", legacy = true, restart = true, rerun = string.find(t, "run it again", 1, true) ~= nil }
	end
	local q_from, q_chat, q_body = string.match(t, "^%[Quoted message from Zeron chat (.-)%. Reply to it with the Zeron `send_message` tool, chat ([^%]]-)%.%]\n\n(.*)$")
	if q_from then
		return { kind = "quoted", legacy = true, from = q_from, chat = q_chat, body = q_body }
	end
	return nil
end

-- A message as the engine said it: its kind and ids (`injected`, the
-- engine's), its words read from its text (which the agent was sent); or,
-- without `injected` (an older engine's), all of it read from its text.
function M.read(text: string?, injected: Injected?): Injected?
	local said = M.parse(text)
	if not injected then
		return said
	end
	-- (Every block but a scheduled wake's shows less than its text: one
	-- whose text isn't what its kind says shows all of it, as words, so
	-- what's hidden is only ever the engine's own.)
	local agrees = said ~= nil and said.kind == injected.kind
	if not agrees and injected.kind ~= "scheduled" then
		return nil
	end
	local out: Injected = {}
	if said and agrees then
		for k, v in said do
			out[k] = v
		end
	end
	for k, v in injected do
		out[k] = v
	end
	out.legacy = nil
	-- (A scheduled wake's own prompt: its words are the block's.)
	if out.kind == "scheduled" and not (said and said.kind == "scheduled") then
		out.prompt = text
	end
	return out
end

-- The kinds this build draws (a newer engine's other kinds show as their
-- text).
local KNOWN = {
	crew_ask = true, answer = true, for_thread = true, on_card = true, changes = true, retraction = true,
	board_check = true, resumed = true, scheduled = true, dump = true, crew_rules = true, quoted = true,
}
function M.known(i: Injected?): Injected?
	return if i and KNOWN[i.kind] then i else nil
end

-- The kinds that stay a bubble (their words are a person's or a sender's),
-- with what shows over it.
local BUBBLES = { dump = true, crew_rules = true, quoted = true }
function M.bubble(i: Injected?): Injected?
	return if i and BUBBLES[i.kind] then i else nil
end

-- A row's height, its corner and how many show folded: Crew updates'.
local ROW_H = 30
local ROW_RADIUS = "$radius.md"

type Row = {
	key: string,
	title: string?,
	chip: { key: string, tone: Badge.Tone }?,
	text: (() -> string) | string,
	at: number?,
	open: (() -> ())?,
	label: string?,
	-- A second way in, at its end (an ask's thread, at its question).
	aside: { icon: Icon.Name, label: string, run: () -> () }?,
}

local function chip_node(chip: () -> { key: string, tone: Badge.Tone }?): Node
	return ui.presence(function(): string
		local c = chip()
		return if c then `{c.key}:{c.tone}` else ""
	end, function(k: string): Node
		local c = chip()
		if not c then
			return ui.div {}
		end
		return Badge.Badge {
			locale.t(c.key),
			tone = c.tone,
			size = "sm",
			class = "orch-injected-state",
		}
	end, { shrink = 0 })
end

local function row_node(r: Row, chip: (() -> { key: string, tone: Badge.Tone }?)?): Node
	local hovered = ui.use_state(false)
	local items: { Node } = {}
	if r.title then
		table.insert(items, ui.text {
			r.title,
			literal = true,
			shrink = 1,
			min_w = 0,
			max_w = 200,
			truncate = true,
			text_size = "$font.size.sm",
			weight = "$font.weight.medium",
			color = "$text.primary",
		})
	end
	if chip or r.chip then
		table.insert(items, chip_node(chip or function()
			return r.chip
		end))
	end
	table.insert(items, ui.text {
		r.text,
		literal = true,
		grow = 1,
		min_w = 0,
		truncate = true,
		text_size = "$font.size.sm",
		color = "$text.secondary",
	})
	if r.at then
		local at = r.at :: number
		table.insert(items, ui.text {
			function(): string
				return status.ago(status.since(at))
			end,
			shrink = 0,
			tabular = true,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		})
	end
	if r.aside then
		local aside = r.aside :: { icon: Icon.Name, label: string, run: () -> () }
		table.insert(items, ui.button {
			class = "orch-injected-aside",
			label = aside.label,
			tooltip = aside.label,
			w = 22,
			h = 22,
			shrink = 0,
			justify = "center",
			items = "center",
			rounded = "$radius.sm",
			color = "$text.tertiary",
			bg = "#00000000",
			hover = { bg = "$hover", color = "$text.secondary" },
			on_click = aside.run,
			Icon { name = aside.icon, size = 14 },
		})
	end
	local base: { [any]: any } = {
		class = "orch-injected-row",
		w_full = true,
		h = ROW_H,
		px = "$space.2",
		gap = "$space.2",
		items = "center",
		rounded = ROW_RADIUS,
	}
	for i, n in items do
		base[i] = n
	end
	if not r.open then
		return ui.row(base)
	end
	base.label = r.label
	base.bg = function(): string
		return if hovered:get() then "$hover" else "#00000000"
	end
	base.hover = {}
	base.active = { bg = "$pressed" }
	base.on_hover = function(inside: boolean)
		hovered:set(inside)
	end
	base.on_click = r.open
	return ui.button(base)
end

-- The block: the orchestrator's thread down its side, a quiet head, then
-- its rows.
local function frame(kind: string, icon: Icon.Name, head: Node, rows: { Node }): Node
	local column: { [any]: any } = {
		grow = 1,
		min_w = 0,
		gap = "$zeron.list.gap",
		head,
	}
	for _, r in rows do
		table.insert(column, r)
	end
	return ui.row {
		class = `orch-injected orch-injected-{kind}`,
		w_full = true,
		gap = "$space.3",
		items = "stretch",
		ui.div { w = 2, shrink = 0, rounded = "$radius.full", bg = "$border" },
		ui.column(column),
	}
end

local function head(icon: Icon.Name, words: any, meta: string?, at: number?): Node
	return ui.row {
		w_full = true,
		h = 24,
		px = "$space.2",
		gap = "$space.2",
		items = "center",
		color = "$text.tertiary",
		Icon { name = icon, size = 14 },
		ui.text {
			words,
			text_size = "$font.size.xs",
			weight = "$font.weight.medium",
			color = "$text.tertiary",
		},
		if meta
			then ui.text {
				meta,
				literal = true,
				tabular = true,
				truncate = true,
				min_w = 0,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			}
			else nil,
		if at
			then ui.div { grow = 1 }
			else nil,
		if at
			then ui.text {
				function(): string
					return status.ago(status.since(at :: number))
				end,
				shrink = 0,
				tabular = true,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			}
			else nil,
	}
end

local function one_line(s: string?): string
	return (string.gsub(s or "", "%s+", " "))
end

-- One card for every ask said into a conversation: a crew ask (`[Crew
-- ask]`, to its orchestrator) and its answer (`[Answer from you]`, `[Answer
-- from your orchestrator]`, to the thread that asked), a decision or an
-- approval alike. It reads the ask as it stands now, by its id, when
-- orchestration has it, else what the message says: open, deferred, or
-- answered (by whom, when, and which option or what words).

-- "Option 2: Tabs — keep it simple": 2, "Tabs", "keep it simple". Words
-- alone: nil, nil, the words.
local function chosen_of(answer: string?): (number?, string?, string?)
	if not answer or answer == "" then
		return nil, nil, nil
	end
	local n, rest = string.match(answer, "^Option (%d+): (.*)$")
	if not n then
		return nil, nil, answer
	end
	local label, extra = string.match(rest :: string, "^(.-) \u{2014} (.*)$")
	return tonumber(n), label or rest, extra
end

type AskView = {
	id: string?,
	state: string,
	question: string,
	options: { string },
	answer: string?,
	via: string?,
	answered: number?,
	deferred: number?,
	chat: string?,
	turn: string?,
}

local function view_of(i: Injected, a: any?): AskView
	local options: { string } = {}
	if a and a.options and #a.options > 0 then
		for _, o in a.options do
			table.insert(options, o.label)
		end
	else
		for _, o in i.options or {} do
			table.insert(options, o)
		end
	end
	local state, via, answer = "open", nil, nil
	if a then
		state, via, answer = a.state or "open", a.answered_via, a.answer
		if state == "open" and a.deferred_until and a.deferred_until > os.time() then
			state = "deferred"
		end
	elseif i.kind == "answer" then
		state, via, answer = "answered", if i.by == "orchestrator" then "orchestrator" else "person", i.answer
	end
	return {
		id = (a and a.id) or i.ask,
		state = state,
		-- (Its markdown's emphasis is the overlay's: a line here.)
		question = (string.gsub(one_line(if a and a.question then a.question else i.question), "%*%*", "")),
		options = options,
		answer = answer,
		via = via,
		answered = a and a.answered,
		deferred = a and a.deferred_until,
		chat = i.chat or (a and a.from_chat),
		turn = i.turn or (a and a.turn_id),
	}
end

local function clock(t: number): string
	return locale.when(t)
end

-- Its state's chip.
local function state_chip(v: AskView): (string, { [string]: any }?, Badge.Tone)
	if v.state == "answered" then
		if v.via == "orchestrator" then
			return "orch.injected.ask.by_orchestrator", nil, "info"
		elseif v.via == "thread" then
			return "orch.injected.ask.in_thread", nil, "success"
		elseif v.via == "action" then
			return "orch.injected.ask.acted", nil, "success"
		elseif v.via == "resolved" then
			-- (Raised by a board check and closed by the engine once what it
			-- asked about cleared: no one answered it.)
			return "orch.injected.ask.resolved", nil, "neutral"
		end
		return "orch.injected.ask.by_you", nil, "success"
	elseif v.state == "dismissed" then
		return "orch.injected.ask.dismissed", nil, "neutral"
	elseif v.state == "deferred" then
		return "orch.injected.ask.deferred_until", { time = clock(v.deferred or os.time()) }, "neutral"
	end
	return "orch.injected.ask.open", nil, "warning"
end

local AskCard = ui.component(function(p: { injected: Injected, at: number?, props: Props }): Node
	local i, props = p.injected, p.props
	local function ask(): any?
		local id = i.ask
		if id and props.ask_of then
			return props.ask_of(id)
		end
		-- (An answer an older engine sent names no ask: its question finds it.)
		if i.question and props.ask_by_question then
			return props.ask_by_question(i.question)
		end
		return nil
	end
	local view = ui.computed(function(): AskView
		return view_of(i, ask())
	end)
	local first = view:peek()
	local chat = first.chat
	local thread = (function(): string
		local t = if chat and props.title_of then props.title_of(chat) else nil
		return if t and t ~= "" then t else (i.thread or locale.s("orch.reports.thread"))
	end)()
	-- (An older engine's answer names no thread: none said.)
	local named = chat ~= nil or i.thread ~= nil
	local hovered = ui.use_state(false)

	local function option_chips(): Node
		return ui.presence(function(): string
			local v = view:get()
			return `{v.state}|{v.answer or ""}|{table.concat(v.options, "\u{1F}")}`
		end, function(_k: string): Node
			local v = view:peek()
			local n, label, extra = chosen_of(if v.state == "answered" then v.answer else nil)
			local chips: { [any]: any } = { gap = "$space.1", items = "center", min_w = 0 }
			if #v.options > 0 then
				for k, o in v.options do
					local picked = n == k
					table.insert(chips, Badge.Badge {
						o,
						tone = if picked then "accent" else "neutral",
						look = if picked then "soft" else "outline",
						icon = if picked then "check" else nil,
						size = "sm",
						max_w = 220,
						class = if picked then "orch-ask-picked" else "orch-ask-option",
					})
				end
			elseif label then
				table.insert(chips, Badge.Badge { label, tone = "accent", icon = "check", size = "sm", max_w = 320, class = "orch-ask-picked" })
			end
			local words = extra or (if n then nil else (if v.state == "answered" then v.answer else nil))
			local out: { [any]: any } = { w_full = true, gap = "$space.1" }
			if #chips > 0 then
				table.insert(out, ui.row(chips))
			end
			if words and words ~= "" then
				table.insert(out, ui.row {
					w_full = true,
					gap = "$space.1",
					items = "start",
					color = "$text.tertiary",
					ui.div { pt = ui.raw(2, "the icon on the words' first line"), shrink = 0, Icon { name = "chat-round-line", size = 12 } },
					ui.text {
						one_line(words),
						literal = true,
						grow = 1,
						min_w = 0,
						wrap = true,
						class = "orch-ask-reply",
						text_size = "$font.size.sm",
						color = "$text.secondary",
					},
				})
			end
			return ui.column(out)
		end, { w_full = true })
	end

	local head_row = ui.row {
		w_full = true,
		h = 24,
		px = "$space.2",
		gap = "$space.2",
		items = "center",
		color = "$text.tertiary",
		ui.presence(function(): string
			return view:get().state
		end, function(state: string): Node
			local done = state == "answered" or state == "dismissed"
			return ui.row {
				gap = "$space.2",
				items = "center",
				shrink = 0,
				Icon { name = if done then "check-circle" else "status-input", size = 14 },
				ui.text {
					locale.t(if done then "orch.injected.decision" else "orch.injected.needs_you"),
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
				},
			}
		end, { shrink = 0 }),
		if named then ui.text { "\u{00B7}", text_size = "$font.size.xs", shrink = 0 } else nil,
		ui.text {
			if named then thread else "",
			literal = true,
			truncate = true,
			min_w = 0,
			shrink = 1,
			text_size = "$font.size.xs",
		},
		ui.div { grow = 1 },
		ui.presence(function(): string
			local key, args = state_chip(view:get())
			return `{key}|{if args then args.time else ""}`
		end, function(_k: string): Node
			local key, args, tone = state_chip(view:peek())
			return Badge.Badge { locale.t(key, args), tone = tone, size = "sm", class = "orch-ask-state" }
		end, { shrink = 0 }),
		ui.text {
			function(): string
				local v = view:get()
				local t = if v.state == "answered" and v.answered then v.answered else p.at
				return if t then status.ago(status.since(t)) else ""
			end,
			shrink = 0,
			tabular = true,
			text_size = "$font.size.xs",
		},
		if chat and props.open_chat
			then ui.button {
				class = "orch-injected-aside",
				label = locale.s("ds.orch.decide.open_thread_named", { name = thread }),
				tooltip = locale.s("ds.orch.decide.open_at"),
				w = 22,
				h = 22,
				shrink = 0,
				justify = "center",
				items = "center",
				rounded = "$radius.sm",
				color = "$text.tertiary",
				bg = "#00000000",
				hover = { bg = "$hover", color = "$text.secondary" },
				on_click = function()
					local go: any = props.open_chat
					go(chat, view:peek().turn)
				end,
				Icon { name = "arrow-up-right", size = 14 },
			}
			else nil,
	}

	local body = ui.button {
		class = "orch-ask-body",
		label = locale.s("orch.injected.ask.open_label", { title = thread }),
		w_full = true,
		px = "$space.2",
		py = "$space.1",
		gap = "$space.2",
		items = "start",
		direction = "column",
		rounded = ROW_RADIUS,
		bg = function(): string
			return if hovered:get() then "$hover" else "#00000000"
		end,
		hover = {},
		active = { bg = "$pressed" },
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		on_click = function()
			local v = view:peek()
			if v.id and props.open_ask then
				props.open_ask(v.id)
			elseif chat and props.open_chat then
				props.open_chat(chat, v.turn)
			end
		end,
		ui.text {
			function(): string
				return view:get().question
			end,
			literal = true,
			w_full = true,
			wrap = true,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			weight = "$font.weight.medium",
			color = "$text.primary",
		},
		option_chips(),
	}
	return frame(i.kind, "status-input", head_row, { body })
end)

M.Block = ui.component(function(props: Props): Node
	local i = props.injected
	local at = props.at
	local function title(chat: string?, fallback: string?): string
		local t = if chat and props.title_of then props.title_of(chat) else nil
		return if t and t ~= "" then t else (fallback or locale.s("orch.reports.thread"))
	end
	local function open_chat(chat: string?, message: string?): (() -> ())?
		if not chat or not props.open_chat then
			return nil
		end
		local open = props.open_chat :: (string, string?) -> ()
		return function()
			open(chat :: string, message)
		end
	end

	if i.kind == "crew_ask" or i.kind == "answer" then
		return AskCard { injected = i, at = at, props = props }
	elseif i.kind == "for_thread" then
		local thread = title(i.chat, i.thread)
		return frame("for_thread", "arrow-right", head("arrow-right", locale.t("orch.injected.for_thread")), {
			row_node({
				key = "relay",
				title = thread,
				text = one_line(i.message),
				at = at,
				open = open_chat(i.chat),
				label = thread,
			}),
		})
	elseif i.kind == "on_card" then
		return frame("on_card", "tag", head("tag", locale.t("orch.injected.on_card")), {
			row_node({ key = "card", title = i.card, text = one_line(i.message), at = at }),
		})
	elseif i.kind == "changes" then
		return frame("changes", "pen", head("pen", locale.t("orch.injected.changes")), {
			row_node({ key = "card", title = i.card, text = one_line(i.text), at = at }),
		})
	elseif i.kind == "retraction" then
		return frame("retraction", "restart", head("restart", locale.t("orch.injected.retraction")), {
			row_node({
				key = "note",
				chip = { key = "orch.injected.retracted", tone = "neutral" },
				text = `\u{201C}{one_line(i.excerpt)}\u{201D}`,
				at = at,
			}),
		})
	elseif i.kind == "board_check" then
		local rows: { Node } = {}
		for n, c in i.cards or {} do
			table.insert(rows, row_node({
				key = c.id or tostring(n),
				title = c.title,
				chip = { key = "orch.injected.check", tone = "warning" },
				text = one_line(c.note),
			}))
		end
		return frame("board_check", "checklist", head("checklist", locale.t("orch.injected.board_check"), tostring(#(i.cards or {})), at), rows)
	elseif i.kind == "resumed" then
		if i.restart then
			return frame("resumed", "action-play", head("action-play", locale.t("orch.injected.resumed")), {
				row_node({
					key = "restart",
					title = locale.s("orch.injected.resumed_restart"),
					text = locale.s(if i.rerun then "orch.injected.resumed_rerun" else "orch.injected.resumed_text"),
					at = at,
				}),
			})
		end
		return frame("resumed", "action-play", head("action-play", locale.t("orch.injected.resumed")), {
			row_node({ key = "card", title = i.card, text = locale.s("orch.injected.resumed_text"), at = at }),
		})
	elseif i.kind == "scheduled" then
		local prompt = i.prompt and trim(i.prompt) or ""
		if prompt == "" then
			return frame("scheduled", "clock-circle", head("clock-circle", locale.t("orch.injected.scheduled"), nil, at), {})
		end
		return frame("scheduled", "clock-circle", head("clock-circle", locale.t("orch.injected.scheduled")), {
			row_node({ key = "prompt", text = one_line(prompt), at = at }),
		})
	end
	return ui.div {}
end)

-- Over a bubble: its crew rules, folded (a row that opens them), or who a
-- quoted message names.
M.Over = ui.component(function(props: { injected: Injected }): Node
	local i = props.injected
	if i.kind == "crew_rules" then
		local open = ui.use_state(false)
		return ui.column {
			class = "orch-injected-rules",
			w_full = true,
			items = "end",
			ui.button {
				label = function(): string
					return locale.s("orch.injected.crew_rules", { name = i.orchestrator or "" })
				end,
				expanded = function(): boolean
					return open:get()
				end,
				h = 24,
				px = "$space.2",
				gap = "$space.1",
				items = "center",
				rounded = ROW_RADIUS,
				color = "$text.tertiary",
				bg = "#00000000",
				hover = { bg = "$hover", color = "$text.secondary" },
				on_click = function()
					open:set(not open:peek())
				end,
				Icon { name = "shield-check", size = 14 },
				ui.text {
					locale.t("orch.injected.crew_rules", { name = i.orchestrator or "" }),
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
				},
				ui.div {
					rotate = ui.spring(open, { from = 0, to = 90, stiffness = 380, damping = 30 }),
					Icon { name = "alt-arrow-right", size = 12 },
				},
			},
			ui.show(function(): boolean
				return open:get()
			end, ui.text {
				i.rules or "",
				literal = true,
				max_w = "80%",
				px = "$space.2",
				pb = "$space.1",
				text_size = "$font.size.sm",
				color = "$text.secondary",
			}),
		}
	elseif i.kind == "quoted" then
		-- (A header copied into another message: who it names, never as the
		-- one who sent it.)
		return ui.row {
			class = "orch-injected-quoted",
			w_full = true,
			justify = "end",
			gap = "$space.1",
			items = "center",
			color = "$text.tertiary",
			selectable = false,
			Icon { name = "chat-round-line", size = 14 },
			ui.text {
				locale.t("orch.injected.quoted", { name = i.from or "" }),
				truncate = true,
				min_w = 0,
				text_size = "$font.size.xs",
				weight = "$font.weight.medium",
			},
		}
	end
	return ui.div {}
end)

return M

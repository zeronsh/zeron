-- Needs you: everything waiting on the person, one list across every
-- orchestrator: the asks they and their crews raised (a question, a
-- decision, an approval, a blocker), and chats whose agent stopped to ask
-- (awaiting input), one row a chat; a card in the board's Needs you is one
-- row, its first ask, with how many more it has (data.needs). Urgent (or
-- gone stale unanswered) first, then the newest, as the board's column; a
-- deferred ask stays out of it until its time (data.needs). An ask is
-- compact here: its question in a line and its options as chips
-- (ds/orch_choices), the recommended one ringed; a chip answers at once,
-- with Undo for a moment before it is sent. The question, or the expand at
-- the chips' end, opens the whole of it (ds/orch_decision); the thread's
-- name opens the thread at the message that asked. A chat awaiting input
-- opens. An answer the engine took says so a moment, then slides out, the
-- rows under it closing up (one it turned down stays, saying why:
-- ds/orch_choices); what was answered lately is folded at the foot, in
-- Recent answers.
--
-- Two or more chats awaiting input with no ask to show are one line, "n
-- chats waiting for input", which opens to their rows. Past `limit` rows,
-- "n more" opens the rest in a list of its own that scrolls past about
-- SCROLL_ROWS rows: the widget never grows without end.
--
--   Asks {
--     needs = function() return data.needs(now, sessions_now) end,
--     recent = function() return decide.recent(now.asks, 5) end,
--     name_of = function(chat_id) return "Orchestrators" end,
--     on_answer = function(ask, { option?, text? }) return promise end,
--     on_dismiss = function(ask) end,
--     on_open = function(chat_id, message_id?) end,
--     on_expand = function(ask) end,               -- the overlay
--     limit = 6,                                  -- rows before "n more"
--   }

local Button = require("@self/button")
local IconButton = require("@self/icon_button").IconButton
local status = require("@self/status")
local locale = require("@self/locale")
local motion = require("@self/motion")
local control = require("@self/control")
local hue = require("@self/orch_hue")
local Badge = require("@self/badge")
local hold = require("@self/orch_hold")
local Ref = require("@self/orch_ref")
local decide = require("@self/orch_decide")
local Choices = require("@self/orch_choices")
local FoldScroll = require("@self/fold_scroll")
local Icon = require("@self/icon")

export type Props = {
	needs: () -> { any },
	recent: (() -> { any })?,
	name_of: (id: string?) -> string?,
	on_answer: (ask: any, choice: decide.Choice) -> any,
	on_dismiss: (ask: any) -> (),
	on_open: (chat_id: string, message: string?) -> (),
	on_expand: ((ask: any) -> ())?,
	limit: number?,
}

-- How long an answered row takes to leave.
local LEAVE_MS = 220
-- The rows "n more" shows before its list scrolls.
local SCROLL_ROWS = 10
-- A chat awaiting input's row, about (its head and its padding).
local INPUT_ROW_H = 52

-- `**bold**` and `` `code` `` in a question, as its runs; the rest plain.
local function runs_of(text: string): { any }
	local out: { any } = {}
	local bold, code = false, false
	local at = 1
	while at <= #text do
		local s, e = string.find(text, if code then "`" else "%*%*", at)
		local tick = if code then nil else string.find(text, "`", at, true)
		if tick and (not s or tick < s) then
			s, e = tick, tick
		end
		local piece = string.sub(text, at, (s or #text + 1) - 1)
		if piece ~= "" then
			table.insert(out, ui.span {
				piece,
				literal = true,
				weight = if bold then 600 else nil,
				font = if code then "$font.mono" else nil,
			})
		end
		local stop: number? = e
		if not s or not stop then
			break
		end
		if string.sub(text, s, stop) == "`" then
			code = not code
		else
			bold = not bold
		end
		at = stop + 1
	end
	return out
end

-- The question's words alone, its marks gone (what it's called).
local function plain(text: string): string
	return (string.gsub(string.gsub(text, "%*%*", ""), "`", ""))
end

-- Two plain values (tables field by field) the same.
local function same(a: any, b: any): boolean
	if a == b then
		return true
	end
	if type(a) ~= "table" or type(b) ~= "table" then
		return false
	end
	for k, v in a do
		if not same(v, b[k]) then
			return false
		end
	end
	for k in b do
		if a[k] == nil then
			return false
		end
	end
	return true
end

-- A need as its row reads it: what it shows, never the whole chat (whose
-- every change, its activity, its title, would wake the row).
local function shown_of(n: any): any
	return {
		id = n.id,
		kind = n.kind,
		urgent = n.urgent,
		at = n.at,
		ask = n.ask,
		input = n.input,
		from = n.from,
		hue = n.hue,
		card = n.card,
		title = n.title,
		asks = n.asks,
		status = n.status,
	}
end

-- A card's row as it shows now: the first of its asks not answered here
-- and taken (the next one comes up as one goes), and how many after it.
-- The same table while that holds (`chosen`, by the row).
local chosen: { [any]: { at: number, v: any } } = setmetatable({}, { __mode = "k" }) :: any
local function card_now(v: any, pending: { [string]: any }): any
	local asks = v.asks
	if v.kind ~= "card" or not asks or #asks == 0 then
		return v
	end
	local at = #asks
	for i, a in asks do
		local p = pending[a.id]
		if not p or p.phase ~= "done" then
			at = i
			break
		end
	end
	local had = chosen[v]
	if had and had.at == at then
		return had.v
	end
	local out = table.clone(v)
	out.ask = asks[at]
	out.more = #asks - at
	chosen[v] = { at = at, v = out }
	return out
end

-- Each kind of ask as Zeron's status says it: a question or a decision
-- waits on input, an approval on approval, a blocker has failed.
local KIND_STATUS: { [string]: status.Kind } = {
	question = "input",
	decision = "input",
	approval = "approval",
	blocker = "failed",
	input = "input",
}

-- A Needs-you card's statuses that have words of their own (the board's).
local CARD_STATUS = { asked = true, failed = true, review = true, stalled = true }

return ui.component(function(props: Props): Node
	-- Rows answered here, kept while they leave: by id, the need as it was.
	local leaving = ui.use_state({} :: { [string]: any })
	-- Where each row last stood, so one leaving keeps its place, and each
	-- row as last seen (one answered here, kept while it says so).
	local last_order: { string } = {}
	local seen: { [string]: any } = {}
	-- A card's asks, to its row's id.
	local by_ask: { [string]: string } = {}
	local expanded = ui.use_state(false)
	local recent_open = ui.use_state(false)
	local limit = props.limit or 6

	-- Each row's need as last handed out, by id: the same table while what
	-- it shows is, so a new ask (or one answered) touches its own row only.
	local kept: { [string]: any } = {}
	local all = ui.computed(function(): { any }
		local now = {}
		local fresh: { [string]: any } = {}
		for _, n in props.needs() do
			local v = shown_of(n)
			local was = kept[n.id]
			if was and same(was, v) then
				v = was
			end
			fresh[n.id] = v
			table.insert(now, v)
		end
		kept = fresh
		local gone = leaving:get()
		local pending = decide.pending:get()
		for i, v in now do
			now[i] = card_now(v, pending)
		end
		local function phase(n: any): string?
			local p = if n and n.ask then pending[n.ask.id] else nil
			return if p then p.phase else nil
		end
		local present: { [string]: boolean } = {}
		local out = {}
		for _, n in now do
			for _, a in n.asks or {} do
				by_ask[a.id] = n.id
			end
			-- (Answered here and said: out, unless it's leaving.)
			if phase(n) ~= "done" or gone[n.id] then
				present[n.id] = true
				seen[n.id] = n
				table.insert(out, n)
			end
		end
		-- A row the list no longer has, leaving or answered here and still
		-- saying so (the engine's state came first): back at its old place.
		for i, id in last_order do
			local n = gone[id] or seen[id]
			local ph = phase(n)
			if n and not present[id] and (gone[id] or (ph ~= nil and ph ~= "done")) then
				table.insert(out, math.min(i, #out + 1), n)
			end
		end
		local order = {}
		for _, n in out do
			table.insert(order, n.id)
		end
		last_order = order
		return out
	end)
	-- Chats awaiting input with nothing to show but that: one line for
	-- them all (two or more), where the first of them stood.
	local inputs_open = ui.use_state(false)
	local function bare_input(n: any): boolean
		return n.kind == "input" and n.ask == nil
	end
	local grouped = ui.computed(function(): { any }
		local list = all:get()
		local inputs = {}
		for _, n in list do
			if bare_input(n) then
				table.insert(inputs, n)
			end
		end
		if #inputs < 2 then
			return list
		end
		local out = {}
		for _, n in list do
			if not bare_input(n) then
				table.insert(out, n)
			elseif n == inputs[1] then
				table.insert(out, { id = "inputs", kind = "inputs", inputs = inputs })
			end
		end
		return out
	end)
	local rows = ui.computed(function(): { any }
		local list = grouped:get()
		if expanded:get() or #list <= limit then
			return list
		end
		local out = {}
		for i = 1, limit do
			out[i] = list[i]
		end
		return out
	end)

	local function leave(n: any)
		leaving:set_key({ n.id }, n)
		ui.after(motion.ms(LEAVE_MS) + 40, function()
			leaving:set_key({ n.id }, nil)
		end)
	end
	-- A card's row whose ask was answered here and taken: another of its
	-- asks to show next (then it stays, showing that one).
	local function next_of(n: any, ask_id: string, pending: { [string]: any }): any?
		for _, a in n.asks or {} do
			local p = pending[a.id]
			if a.id ~= ask_id and (not p or p.phase ~= "done") then
				return a
			end
		end
		return nil
	end
	-- An answer the engine took, said for a moment: the row slides out.
	local left: { [string]: boolean } = {}
	ui.effect(function()
		local pending = decide.pending:get()
		for ask_id, p in pending do
			local key = `ask:{ask_id}`
			if p.phase == "done" and not left[key] then
				left[key] = true
				local n = seen[key] or (by_ask[ask_id] and seen[by_ask[ask_id]])
				if n and not next_of(n, ask_id, pending) then
					leave(n)
				end
			elseif p.phase ~= "done" then
				left[key] = nil
			end
		end
		for key in left do
			if not pending[string.sub(key, 5)] then
				left[key] = nil
			end
		end
	end)

	local function row(_n: any, _i: number, item: State<any>): Node
		local function is_leaving(): boolean
			return leaving:get()[item:get().id] ~= nil
		end
		local function ask(): any
			return item:get().ask
		end
		-- Sent past its Undo: the engine's reply (the row goes once it's
		-- taken, ds/orch_decide).
		local function commit(a: any, choice: decide.Choice): any
			return props.on_answer(a, choice)
		end
		local function expand()
			local a = ask()
			if a and props.on_expand then
				props.on_expand(a)
			end
		end
		-- Into the thread, at the message that asked (an ask's), else its end.
		local function open_thread()
			local n = item:peek()
			if n.from then
				local a = n.ask
				props.on_open(n.from, a and a.turn_id or nil)
			end
		end
		-- Whose: a card's title, else the thread's name.
		local function from_name(): string
			local n = item:get()
			return n.title or props.name_of(n.from) or ""
		end
		local function kind(): string
			local n = item:get()
			if n.kind == "card" and not n.ask then
				return if n.status == "failed" then "blocker" else "input"
			end
			return if n.kind == "input" then "input" else ((n.ask and n.ask.kind) or "question")
		end
		-- What its head calls it: the ask's kind, or a card with none (its
		-- thread waiting, nothing moving) as the board says it stands.
		local function kind_word(): string
			local n = item:get()
			if n.kind == "card" and not n.ask and CARD_STATUS[n.status or ""] then
				return locale.s(`orch.board.status.{n.status}`)
			end
			return locale.s(`ds.orch.ask.{kind()}`)
		end
		local function more(): number
			return item:get().more or 0
		end
		-- Dragged by its head or its question: a reference to the ask (a chat
		-- awaiting input, to the chat), for Capture (ds/orch_ref).
		local drag = Ref.source(function(): any
			local n = item:peek()
			local a = n.ask
			if a and type(a.id) == "string" then
				local options: { string } = {}
				for _, o in a.options or {} do
					if type(o.label) == "string" then
						table.insert(options, o.label)
					end
				end
				return Ref.make("ask", a.id, a.question, {
					chat = a.from_chat or n.from,
					orchestrator = a.orchestrator_id,
					options = if #options > 0 then table.concat(options, " / ") else nil,
				})
			end
			if type(n.from) == "string" then
				return Ref.make("chat", n.from, props.name_of(n.from), { state = "input" })
			end
			return nil
		end)
		local gone = ui.computed(is_leaving)
		local state = hold.value(function(): status.Kind
			return KIND_STATUS[kind()] or "input"
		end)
		return ui.column {
			class = "orch-need",
			w_full = true,
			gap = "$space.2",
			py = "$space.3",
			layout = "position",
			layout_duration = 200,
			opacity = motion.tween(ui.computed(function(): number
				return if gone:get() then 0 else 1
			end), { duration = LEAVE_MS, ease = "inCubic" }),
			x = motion.tween(ui.computed(function(): number
				return if gone:get() then 28 else 0
			end), { duration = LEAVE_MS, ease = "inCubic" }),
			-- Who asks, what kind of ask, how long ago.
			ui.row {
				class = "orch-need-head",
				on_drag = drag.on_drag,
				drag_preview = drag.drag_preview,
				drag_preview_anchor = drag.drag_preview_anchor,
				drag_preview_offset = drag.drag_preview_offset,
				w_full = true,
				gap = "$space.2",
				items = "center",
				status.StatusIcon { kind = state, size = 14 },
				ui.text {
					kind_word,
					shrink = 0,
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
					color = function(): string
						return `$zeron.status.{state()}`
					end,
				},
				ui.show(function(): boolean
					return item:get().urgent == true
				end, Badge.Badge { locale.s("ds.orch.ask.urgent"), tone = "danger", size = "sm" }),
				-- Whose: the thread, opened at the message that asked.
				ui.button {
					class = "orch-need-from",
					label = function(): string
						-- (A card asking nothing: its thread, at its end.)
						if item:get().kind == "card" and not ask() then
							return locale.s("orch.board.open_thread_named", { title = from_name() })
						end
						return locale.s("ds.orch.decide.open_thread_named", { name = from_name() })
					end,
					tooltip = locale.s("ds.orch.decide.open_at"),
					tooltip_delay = 500,
					gap = "$zeron.space.tight",
					items = "center",
					min_w = 0,
					shrink = 1,
					px = "$zeron.space.hair",
					rounded = "$radius.sm",
					bg = "#00000000",
					hover = { bg = "$hover" },
					active = { bg = "$pressed" },
					focus = control.ring,
					on_click = open_thread,
					hue.Dot { hue = function(): number?
						return item:get().hue
					end, size = 6 },
					ui.text { from_name, text_size = "$font.size.xs", color = "$text.tertiary", truncate = true, min_w = 0 },
				},
				ui.div { grow = 1 },
				ui.text {
					function(): string
						return status.ago(status.since(item:get().at or 0))
					end,
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
				-- (A chat awaiting input with no ask of its own: into it.)
				ui.show(function(): boolean
					return ask() == nil or item:get().input == true
				end, Button {
					tone = "secondary",
					size = "xs",
					on_click = open_thread,
					locale.t("ds.orch.ask.open"),
				}),
				ui.show(function(): boolean
					return ask() ~= nil
				end, IconButton {
					icon = "close",
					label = locale.t("ds.orch.ask.dismiss"),
					size = "sm",
					tone = "quiet",
					on_click = function()
						local a = ask()
						if a and not is_leaving() then
							decide.undo(a.id)
							props.on_dismiss(a)
							-- (A card with more asks stays: the next one comes up.)
							if more() == 0 then
								leave(item:peek())
							end
						end
					end,
				}),
			},
			ui.show(function(): boolean
				return ask() ~= nil
			end, ui.column {
				w_full = true,
				gap = "$space.2",
				-- The question, a line of it: all of it in the overlay.
				ui.presence(function(): string
					local n = item:get()
					return if n.ask then `ask:{n.ask.id}` else "input"
				end, function(key: string): Node
					local n = item:peek()
					if not n.ask then
						return ui.div {}
					end
					local runs: any = runs_of(n.ask.question or "")
					runs.truncate = true
					runs.lines = 1
					runs.w_full = true
					runs.min_w = 0
					runs.text_size = "$font.size.md"
					runs.line_height = "$font.line.md"
					runs.color = "$text.primary"
					return ui.button {
						class = "orch-need-question",
						label = plain(n.ask.question or ""),
						w_full = true,
						min_w = 0,
						p = 0,
						items = "center",
						bg = "#00000000",
						hover = { bg = "#00000000" },
						active = { bg = "#00000000" },
						focus = control.ring,
						cursor = "pointer",
						on_click = expand,
						on_drag = drag.on_drag,
						drag_preview = drag.drag_preview,
						drag_preview_anchor = drag.drag_preview_anchor,
						drag_preview_offset = drag.drag_preview_offset,
						ui.rich(runs),
					}
				end, { w_full = true }),
				-- A card asking more than this: how many more, its first shown,
				-- on a line of its own under the question (the card's title
				-- whole in the head, however narrow).
				ui.show(function(): boolean
					return more() > 0
				end, ui.row {
					w_full = true,
					Badge.Badge {
						function(): string
							return locale.s("orch.board.more_asks", { n = more() })
						end,
						class = "orch-need-more",
						size = "sm",
						tabular = true,
					},
				}),
				-- How to answer: its options as chips, else a line to write.
				Choices {
					ask = ask,
					on_answer = commit,
					on_expand = if props.on_expand then function(_a: any)
						expand()
					end else nil,
					line = true,
				},
			}),
		}
	end

	-- The chats awaiting input, one line: how many, opening to their rows.
	local function inputs_row(item: State<any>): Node
		local list = ui.computed(function(): { any }
			return item:get().inputs or {}
		end)
		return ui.column {
			class = "orch-need-inputs",
			w_full = true,
			py = "$space.2",
			gap = "$space.1",
			ui.button {
				class = "orch-need-inputs-toggle",
				label = function(): string
					return locale.s("ds.orch.ask.inputs", { n = #list:get() })
				end,
				expanded = function(): boolean
					return inputs_open:get()
				end,
				w_full = true,
				h = 28,
				px = "$space.2",
				mx = ui.raw(-8, "its words over the rows' words, its wash out past them"),
				gap = "$space.2",
				items = "center",
				rounded = "$radius.md",
				bg = "#00000000",
				hover = { bg = "$hover" },
				active = { bg = "$pressed" },
				focus = control.ring,
				on_click = function()
					inputs_open:set(not inputs_open:peek())
				end,
				status.StatusIcon { kind = "input", size = 14 },
				ui.text {
					function(): string
						return locale.s("ds.orch.ask.inputs", { n = #list:get() })
					end,
					grow = 1,
					min_w = 0,
					truncate = true,
					text_size = "$font.size.sm",
					color = "$text.secondary",
				},
				ui.div {
					color = "$text.tertiary",
					Icon {
						name = function(): any
							return if inputs_open:get() then "alt-arrow-down" else "alt-arrow-right"
						end,
						size = 12,
					},
				},
			},
			-- (Opened: about SCROLL_ROWS of them, then they scroll.)
			ui.show(function(): boolean
				return inputs_open:get()
			end, FoldScroll {
				class = "orch-need-inputs-list",
				open = function(): boolean
					return true
				end,
				max_h = SCROLL_ROWS * INPUT_ROW_H,
				list = function(): Node
					return ui.each(list, function(n: any, i: number, one: State<any>): Node
						return row(n, i, one)
					end, { key = function(n: any): string
						return n.id
					end, w_full = true })
				end,
			}),
		}
	end

	-- Each row's height as laid out, and the list's: what "n more" scrolls in.
	local heights = ui.use_state({} :: { [string]: number })
	local function cap_h(): number
		local h = 0
		local hs = heights:get()
		for i, n in rows:get() do
			if i > SCROLL_ROWS then
				break
			end
			h += hs[n.id] or 0
		end
		return math.max(h, 1)
	end

	local function by_id(n: any): string
		return n.id
	end

	local function rule(): Node
		return ui.div { w_full = true, h = 1, bg = "$border.subtle" }
	end

	-- What was answered lately (the engine's, and what it took here),
	-- folded at the foot: the question, and the answer under it.
	local recent = ui.computed(function(): { any }
		return if props.recent then props.recent() else {}
	end)
	local function recent_row(_r: any, _i: number, r: State<any>): Node
		return ui.column {
			class = "orch-recent-row",
			w_full = true,
			min_w = 0,
			py = "$space.1",
			gap = "$zeron.space.hair",
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				ui.text {
					function(): string
						return plain(r:get().question or "")
					end,
					literal = true,
					truncate = true,
					grow = 1,
					min_w = 0,
					text_size = "$font.size.sm",
					color = "$text.secondary",
				},
				ui.text {
					function(): string
						return status.ago(status.since(r:get().at or 0))
					end,
					tabular = true,
					shrink = 0,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			},
			ui.row {
				w_full = true,
				gap = "$zeron.space.tight",
				items = "center",
				ui.div { color = "$zeron.status.done", shrink = 0, Icon { name = "check", size = 10 } },
				ui.text {
					function(): string
						return r:get().answer or ""
					end,
					literal = true,
					truncate = true,
					min_w = 0,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			},
		}
	end
	local function recent_fold(): Node
		return ui.column {
			class = "orch-recent",
			w_full = true,
			pt = "$space.2",
			gap = "$space.1",
			ui.button {
				class = "orch-recent-toggle",
				label = locale.t("ds.orch.decide.recent"),
				expanded = function(): boolean
					return recent_open:get()
				end,
				h = 28,
				px = "$space.2",
				mx = ui.raw(-8, "its words over the rows' words, its wash out past them"),
				gap = "$space.2",
				items = "center",
				rounded = "$radius.md",
				bg = "#00000000",
				hover = { bg = "$hover" },
				active = { bg = "$pressed" },
				focus = control.ring,
				self = "start",
				on_click = function()
					recent_open:set(not recent_open:peek())
				end,
				ui.div {
					color = "$text.tertiary",
					Icon {
						name = function(): any
							return if recent_open:get() then "alt-arrow-down" else "alt-arrow-right"
						end,
						size = 12,
					},
				},
				ui.text { locale.t("ds.orch.decide.recent"), text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.secondary" },
				ui.text {
					function(): string
						return tostring(#recent:get())
					end,
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			},
			ui.show(function(): boolean
				return recent_open:get()
			end, ui.each(recent, recent_row, {
				key = function(r: any): string
					return r.id
				end,
				w_full = true,
			})),
		}
	end

	return ui.column {
		class = "orch-asks",
		w_full = true,
		ui.show(function(): boolean
			return #all:get() == 0
		end, ui.row {
			class = "orch-asks-empty",
			w_full = true,
			py = "$space.3",
			gap = "$space.2",
			items = "center",
			status.StatusIcon { kind = "done", size = 14 },
			ui.text { locale.t("ds.orch.ask.none"), text_size = "$font.size.sm", color = "$text.tertiary" },
		}),
		-- (Its own scroll once "n more" has opened it: about SCROLL_ROWS
		-- rows tall, each row's height as laid out.)
		FoldScroll {
			class = "orch-asks-list",
			open = function(): boolean
				return expanded:get()
			end,
			max_h = cap_h,
			list = function(): Node
				return ui.each(rows, function(n: any, i: number, item: State<any>): Node
					return ui.column {
						w_full = true,
						layout = "position",
						layout_duration = 200,
						on_resize = function(size: { w: number, h: number })
							local key = item:peek().id
							if math.abs((heights:peek()[key] or 0) - size.h) >= 1 then
								heights:set_key({ key }, size.h)
							end
						end,
						ui.show(function(): boolean
							local top = rows:get()[1]
							return top ~= nil and top.id ~= item:get().id
						end, rule()),
						if n.kind == "inputs" then inputs_row(item) else row(n, i, item),
					}
				end, { key = by_id, w_full = true })
			end,
		},
		ui.show(function(): boolean
			return #grouped:get() > limit
		end, ui.button {
			class = "orch-asks-more",
			label = function(): string
				return if expanded:get() then locale.s("ds.orch.less") else locale.s("ds.orch.more", { n = #grouped:get() - limit })
			end,
			h = 28,
			gap = "$space.2",
			items = "center",
			bg = "#00000000",
			on_click = function()
				expanded:set(not expanded:peek())
			end,
			ui.div {
				color = "$text.tertiary",
				Icon {
					name = function(): any
						return if expanded:get() then "alt-arrow-up" else "alt-arrow-down"
					end,
					size = 12,
				},
			},
			ui.text {
				function(): string
					return if expanded:get() then locale.s("ds.orch.less") else locale.s("ds.orch.more", { n = #grouped:get() - limit })
				end,
				text_size = "$font.size.sm",
				color = "$text.secondary",
			},
		}),
		ui.show(function(): boolean
			return #recent:get() > 0
		end, recent_fold()),
	}
end)

-- Needs you: everything waiting on the person, one list across every
-- orchestrator: the asks they and their crews raised (a question, a
-- decision, an approval, a blocker), and chats whose agent stopped to ask
-- (awaiting input), one row a chat. Urgent (or gone stale unanswered)
-- first, then the one waiting longest. An ask is answered where it
-- stands: its options as buttons, or a line to write in; a thread's
-- question from its turn's end has both, and opens its thread. A chat
-- awaiting input opens. An answered one slides out, the rows under it
-- closing up.
--
--   Asks {
--     needs = function() return data.needs(now, sessions_now) end,
--     name_of = function(chat_id) return "Orchestrators" end,
--     on_answer = function(ask, text) end,
--     on_dismiss = function(ask) end,
--     on_open = function(chat_id) end,
--     limit = 6,                                  -- rows before "n more"
--   }

local Button = require("@self/button")
local IconButton = require("@self/icon_button").IconButton
local TextField = require("@self/text_field")
local status = require("@self/status")
local locale = require("@self/locale")
local motion = require("@self/motion")
local hue = require("@self/orch_hue")
local Badge = require("@self/badge")
local hold = require("@self/orch_hold")
local data = require("@self/orch_data")
local Ref = require("@self/orch_ref")

export type Props = {
	needs: () -> { any },
	name_of: (id: string?) -> string?,
	on_answer: (ask: any, text: string) -> (),
	on_dismiss: (ask: any) -> (),
	on_open: (chat_id: string) -> (),
	limit: number?,
}

-- How long an answered row takes to leave.
local LEAVE_MS = 220

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

-- Each kind of ask as Zeron's status says it: a question or a decision
-- waits on input, an approval on approval, a blocker has failed.
local KIND_STATUS: { [string]: status.Kind } = {
	question = "input",
	decision = "input",
	approval = "approval",
	blocker = "failed",
	input = "input",
}

return ui.component(function(props: Props): Node
	-- Rows answered here, kept while they leave: by id, the need as it was.
	local leaving = ui.use_state({} :: { [string]: any })
	-- Where each row last stood, so one leaving keeps its place.
	local last_order: { string } = {}
	local expanded = ui.use_state(false)
	local limit = props.limit or 6

	local all = ui.computed(function(): { any }
		local now = props.needs()
		local gone = leaving:get()
		local present: { [string]: boolean } = {}
		for _, n in now do
			present[n.id] = true
		end
		local out = table.clone(now)
		-- A leaving row the list no longer has: back at its old place.
		for i, id in last_order do
			local n = gone[id]
			if n and not present[id] then
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
	local rows = ui.computed(function(): { any }
		local list = all:get()
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

	local function row(_n: any, _i: number, item: State<any>): Node
		local draft = ui.use_state("")
		local reply = ui.use_state("")
		local function is_leaving(): boolean
			return leaving:get()[item:get().id] ~= nil
		end
		local function ask(): any
			return item:get().ask
		end
		local function answer(text: string)
			local a = ask()
			if not a or is_leaving() then
				return
			end
			local t = string.match(text, "^%s*(.-)%s*$") or ""
			if t == "" then
				return
			end
			props.on_answer(a, t)
			leave(item:peek())
		end
		local function from_name(): string
			local n = item:get()
			return props.name_of(n.from) or ""
		end
		local function kind(): string
			local n = item:get()
			return if n.kind == "input" then "input" else ((n.ask and n.ask.kind) or "question")
		end
		-- A thread's question from its turn's end: answered here or in it.
		local function turn_end(): boolean
			return data.turn_end(ask())
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
					function(): string
						return locale.s(`ds.orch.ask.{kind()}`)
					end,
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
				ui.row {
					gap = "$zeron.space.tight",
					items = "center",
					min_w = 0,
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
				-- (A chat awaiting input, or a thread that asked at its turn's
				-- end: into it. Its ask, if it has one, shows here too,
				-- answered in place.)
				ui.show(function(): boolean
					return ask() == nil or item:get().input == true or turn_end()
				end, Button {
					tone = "secondary",
					size = "xs",
					on_click = function()
						local n = item:peek()
						if n.from then
							props.on_open(n.from)
						end
					end,
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
							props.on_dismiss(a)
							leave(item:peek())
						end
					end,
				}),
			},
			ui.show(function(): boolean
				return ask() ~= nil
			end, ui.column {
				w_full = true,
				gap = "$space.2",
				-- The question (an ask's), or the chat's title (awaiting input).
				ui.presence(function(): string
					local n = item:get()
					return if n.ask then `ask:{n.ask.id}` else "input"
				end, function(key: string): Node
					local n = item:peek()
					if n.ask then
						local runs: any = runs_of(n.ask.question or "")
						runs.wrap = true
						runs.w_full = true
						runs.text_size = "$font.size.md"
						runs.line_height = "$font.line.md"
						runs.color = "$text.primary"
						return ui.div {
							class = "orch-need-question",
							w_full = true,
							on_drag = drag.on_drag,
							drag_preview = drag.drag_preview,
							drag_preview_anchor = drag.drag_preview_anchor,
							drag_preview_offset = drag.drag_preview_offset,
							ui.rich(runs),
						}
					end
					-- (Awaiting input: its question is in the chat; the head says whose.)
					return ui.div {}
				end, { w_full = true }),
				-- How to answer: the options, a line to write, or the chat.
				ui.switch(function(): string
					local n = item:get()
					if n.kind == "input" then
						return "open"
					end
					return if n.ask and #(n.ask.options or {}) > 0 then "options" else "line"
				end, {
					options = ui.column {
						w_full = true,
						gap = "$space.2",
						ui.each(ui.computed(function(): { any }
							local a = ask()
							return (a and a.options) or {}
						end), function(o: any, oi: number): Node
							return Button {
								tone = if oi == 1 then "secondary" else "ghost",
								size = "sm",
								tooltip = if o.detail and o.detail ~= "" then o.detail else nil,
								on_click = function()
									answer(o.label)
								end,
								o.label,
							}
						end, { direction = "row", flex_wrap = true, gap = "$space.2", items = "center" }),
						-- (From a turn's end: a word of the person's own, too.)
						ui.show(turn_end, TextField {
							value = reply,
							size = "sm",
							w_full = true,
							name = locale.t("ds.orch.ask.reply"),
							placeholder = locale.t("ds.orch.ask.reply"),
							on_commit = function(text: string)
								answer(text)
							end,
						}),
					},
					line = TextField {
						value = draft,
						size = "sm",
						w_full = true,
						name = locale.t("ds.orch.ask.answer"),
						placeholder = locale.t("ds.orch.ask.answer"),
						on_commit = function(text: string)
							answer(text)
						end,
					},
					open = ui.div {},
				}),
			}),
		}
	end

	local function by_id(n: any): string
		return n.id
	end

	local function rule(): Node
		return ui.div { w_full = true, h = 1, bg = "$border.subtle" }
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
		ui.each(rows, function(n: any, i: number, item: State<any>): Node
			return ui.column {
				w_full = true,
				layout = "position",
				layout_duration = 200,
				ui.show(function(): boolean
					local top = rows:get()[1]
					return top ~= nil and top.id ~= item:get().id
				end, rule()),
				row(n, i, item),
			}
		end, { key = by_id, w_full = true }),
		ui.show(function(): boolean
			return #all:get() > limit
		end, ui.button {
			label = function(): string
				return if expanded:get() then locale.s("ds.orch.less") else locale.s("ds.orch.more", { n = #all:get() - limit })
			end,
			h = 28,
			items = "center",
			bg = "#00000000",
			on_click = function()
				expanded:set(not expanded:peek())
			end,
			ui.text {
				function(): string
					return if expanded:get() then locale.s("ds.orch.less") else locale.s("ds.orch.more", { n = #all:get() - limit })
				end,
				text_size = "$font.size.sm",
				color = "$text.secondary",
			},
		}),
	}
end)

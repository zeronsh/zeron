--!strict
-- A card, whole: opened beside the board (ds/orch_board), or in an
-- orchestrator's side panel in place of its tabs. Its title and what it
-- last said; then what the person can do: its ask's own buttons while it
-- needs them (with Snooze), a way back for one put off, else its thread;
-- then its facts (how it stands, whose it is, its threads, what ends it,
-- its branch or pull request); then "What happened", the timeline the
-- engine writes from events (a thread started, each report, asks and
-- answers, each move with why), the newest last.
--
--   Detail.Detail { ctx = ctx, act = act, card = card_id,
--     on_close = fn,          -- its close control (or Back, with `back`)
--     back = true,            -- in a panel: a way back, not a close
--     flat = true }           -- no surface of its own (the panel's)
--
-- By itself, for a host that shows a card's detail in its own place (an
-- orchestrator's side panel): the board's props (ds/orch_board_ctx), with
-- the card's id and the same three:
--
--   Detail.Card { items = fn, cards = fn, asks = fn, …, card = fn -> id?,
--     on_close = fn, back = true, flat = true }

local Icon = require("@self/icon")
local IconButton = require("@self/icon_button").IconButton
local Button = require("@self/button")
local ContextMenu = require("@self/context_menu")
local Choices = require("@self/orch_choices")
local hue = require("@self/orch_hue")
local locale = require("@self/locale")
local material = require("@self/material")
local Status = require("@self/status")
local Ctx = require("@self/orch_board_ctx")
local Actions = require("@self/orch_board_actions")
local Card = require("@self/orch_board_card")

local M = {}

export type Props = {
	ctx: Ctx.Ctx,
	act: Actions.Actions,
	-- Its card's id (not `id`: a component's `id` names its element).
	card: string,
	on_close: (() -> ())?,
	back: boolean?,
	flat: boolean?,
}

-- What ends a card, as its finish line says it.
local function done_when(card: any?): string
	local f = card and card.finish
	local line = f and f.line
	if line == "merged" then
		return if f.into then locale.s("orch.board.finish.merged_into", { branch = f.into }) else locale.s("orch.board.finish.merged")
	elseif line == "pr" then
		return locale.s("orch.board.finish.pr")
	elseif line == "accept" then
		return locale.s("orch.board.finish.accept")
	end
	return locale.s("orch.board.finish.report")
end
M.done_when = done_when

-- A fact's row: its name, then what it says.
local function fact(key: string, shown: (() -> boolean)?, value: Node): Node
	local row = ui.row {
		class = "orch-detail-fact",
		w_full = true,
		min_w = 0,
		min_h = 24,
		gap = "$space.3",
		items = "start",
		ui.text {
			locale.t(key),
			w = 92,
			shrink = 0,
			text_size = "$font.size.sm",
			line_height = "$font.line.md",
			color = "$text.tertiary",
		},
		ui.div { grow = 1, basis = 0, min_w = 0, value },
	}
	return if shown then ui.show(shown, row) else row
end

-- A small heading with its count.
local function heading(key: string, count: (() -> number)?): Node
	return ui.row {
		w_full = true,
		gap = "$space.2",
		items = "center",
		ui.text { locale.t(key), text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.secondary" },
		if count
			then ui.text {
				function(): string
					return tostring(count())
				end,
				tabular = true,
				text_size = "$font.size.sm",
				color = "$text.tertiary",
			}
			else nil,
	}
end

M.Detail = ui.component(function(props: Props): Node
	local ctx, act, id = props.ctx, props.act, props.card
	local entry = ui.computed(function(): Ctx.Entry?
		return ctx.entry(id)
	end)
	local function e(): Ctx.Entry?
		return entry:get()
	end
	local function card(): any?
		local x = e()
		return x and x.card
	end
	local ask = ui.computed(function(): any?
		local x = e()
		return if x and x.place == "needs_you" then x.ask else nil
	end)
	-- What the box under its title is: an ask to answer, a wait to answer in
	-- its thread, an ask put off, or nothing of the person's.
	local needs = ui.computed(function(): string
		local x = e()
		if not x then
			return "none"
		elseif x.place == "needs_you" then
			return if x.ask then "ask" else "thread"
		elseif x.place == "snoozed" then
			return "snoozed"
		end
		return "none"
	end)
	local menu_open = ui.use_state(false)
	local snooze_open = ui.use_state(false)

	local owner = ui.computed(function(): any?
		local x = e()
		return x and ctx.owner(x.item.orchestrator_id)
	end)
	local owner_mark = ui.row {
		class = "orch-detail-owner",
		min_w = 0,
		gap = "$space.2",
		items = "center",
		hue.Dot {
			hue = function(): number?
				local o = owner:get()
				return o and o.hue
			end,
			size = 7,
		},
		ui.text {
			function(): string
				local o = owner:get()
				return if o then o.name or "" else locale.s("orch.board.no_orchestrator")
			end,
			min_w = 0,
			truncate = true,
			text_size = "$font.size.sm",
			color = "$text.secondary",
		},
	}

	local head = ui.row {
		class = "orch-detail-head",
		w_full = true,
		min_w = 0,
		gap = "$space.1",
		items = "center",
		if props.back and props.on_close
			then IconButton {
				id = "orch-detail-back",
				icon = "alt-arrow-left",
				label = locale.t("orch.board.back"),
				size = "md",
				tone = "quiet",
				on_click = props.on_close,
			}
			else nil,
		ui.div { grow = 1, basis = 0, min_w = 0, owner_mark },
		(ContextMenu {
			open = menu_open,
			label = locale.t("orch.board.card_actions"),
			shrink = 0,
			items = function(): { any }?
				local x = entry:peek()
				return if x then act.menu(x) else nil
			end,
			IconButton {
				id = "orch-detail-menu",
				icon = "more-horizontal",
				label = locale.t("orch.board.card_actions"),
				size = "md",
				tone = "quiet",
				lit = menu_open,
				on_click = function()
					menu_open:set(true)
				end,
			},
		}) :: Node,
		if props.on_close and not props.back
			then IconButton {
				id = "orch-detail-close",
				icon = "close",
				label = locale.t("orch.board.close_detail"),
				size = "md",
				tone = "quiet",
				on_click = props.on_close,
			}
			else nil,
	}

	local title = ui.text {
		function(): string
			local x = e()
			return if x then x.item.title or "" else ""
		end,
		id = "orch-detail-title",
		w_full = true,
		wrap = true,
		selectable = true,
		text_size = "$font.size.lg",
		line_height = "$font.line.lg",
		weight = "$font.weight.semibold",
		color = "$text.primary",
	}
	-- What it last said (a running one: what it's doing), never the
	-- question: that's in its box.
	local said = ui.computed(function(): string
		local x = e()
		if not x then
			return ""
		end
		local as_card = table.clone(x) :: any
		as_card.ask = nil
		return Card.summary(ctx, as_card)
	end)
	local summary = ui.show(function(): boolean
		return said:get() ~= ""
	end, ui.text {
		function(): string
			return said:get()
		end,
		id = "orch-detail-summary",
		w_full = true,
		wrap = true,
		lines = 6,
		truncate = true,
		selectable = true,
		text_size = "$font.size.sm",
		line_height = "$font.line.md",
		color = "$text.secondary",
	})

	-- The box of what the person can do now.
	local function box(children: { [any]: any }): Node
		local p: { [any]: any } = {
			id = "orch-detail-needs",
			class = "orch-detail-needs",
			w_full = true,
			min_w = 0,
			p = "$space.3",
			gap = "$space.2",
			rounded = "$radius.lg",
			border = 1,
			border_color = "$zeron.tone.warning.edge",
			bg = "$zeron.tone.warning.wash",
		}
		for k, v in children do
			p[k] = v
		end
		return ui.column(p)
	end
	local function open_thread_button(tone: "primary" | "secondary"): Node
		return Button {
			id = "orch-detail-open",
			size = "sm",
			tone = tone,
			on_click = function()
				local x = entry:peek()
				if x then
					act.open_thread(x)
				end
			end,
			Icon { name = "chat-round-line", size = 14 },
			locale.t("orch.board.open_thread"),
		}
	end
	local needs_box = ui.switch(function(): string
		return needs:get()
	end, {
		ask = function(): Node
			return box {
				ui.text {
					function(): string
						local a = ask:get()
						return if a then Card.plain(a.question or "") else ""
					end,
					id = "orch-detail-question",
					w_full = true,
					wrap = true,
					selectable = true,
					text_size = "$font.size.sm",
					line_height = "$font.line.md",
					weight = "$font.weight.medium",
					color = "$text.primary",
				},
				Choices {
					ask = function(): any
						return ask:get()
					end,
					on_answer = function(a: any, choice: any): any
						return act.answer(a, choice)
					end,
					on_expand = if act.can("expand_ask") then act.expand else nil,
					line = true,
					size = "sm",
					class = "orch-detail-choices",
				},
				ui.row {
					w_full = true,
					gap = "$space.1",
					items = "center",
					if act.can("snooze")
						then (ContextMenu {
							open = snooze_open,
							label = locale.t("orch.board.snooze"),
							shrink = 0,
							items = function(): { any }?
								local a = ask:peek()
								return if a then act.snooze_menu(a) else nil
							end,
							Button {
								id = "orch-detail-snooze",
								size = "xs",
								tone = "ghost",
								on_click = function()
									snooze_open:set(true)
								end,
								Icon { name = "clock-circle", size = 12 },
								locale.t("orch.board.snooze"),
							},
						}) :: Node
						else nil,
					if act.can("open_thread")
						then Button {
							id = "orch-detail-ask-thread",
							size = "xs",
							tone = "ghost",
							on_click = function()
								local x = entry:peek()
								if x then
									act.open_thread(x)
								end
							end,
							Icon { name = "chat-round-line", size = 12 },
							locale.t("orch.board.open_thread"),
						}
						else nil,
				},
			}
		end,
		-- A question its thread asked in its own chat: answered there.
		thread = function(): Node
			return box {
				ui.text {
					function(): string
						local c = card()
						local note = c and c.note
						return if note and note ~= "" then note else locale.s("orch.board.answer_in_thread")
					end,
					w_full = true,
					wrap = true,
					text_size = "$font.size.sm",
					line_height = "$font.line.md",
					weight = "$font.weight.medium",
					color = "$text.primary",
				},
				ui.row { w_full = true, open_thread_button("primary") },
			}
		end,
		snoozed = function(): Node
			return box {
				border_color = "$border.subtle",
				bg = "#00000000",
				ui.row {
					w_full = true,
					min_w = 0,
					gap = "$space.2",
					items = "center",
					ui.div { shrink = 0, color = "$text.tertiary", Icon { name = "clock-circle", size = 14 } },
					ui.text {
						function(): string
							local x = e()
							if x and x.until_at then
								return locale.s("orch.board.snoozed_until", { time = Card.time(x.until_at, ctx.at()) })
							end
							return locale.s("orch.board.place.snoozed")
						end,
						grow = 1,
						min_w = 0,
						truncate = true,
						text_size = "$font.size.sm",
						color = "$text.secondary",
					},
					if act.can("snooze")
						then Button {
							id = "orch-detail-back-now",
							size = "xs",
							tone = "secondary",
							on_click = function()
								local x = entry:peek()
								local a = x and x.asks[1]
								if a then
									act.snooze(a, nil)
								end
							end,
							locale.t("orch.board.bring_back"),
						}
						else nil,
				},
			}
		end,
		none = function(): Node
			-- Its thread, and what a card at rest offers: a paused one its
			-- Resume, a closed one its Reopen.
			return ui.row {
				id = "orch-detail-actions",
				w_full = true,
				gap = "$space.2",
				items = "center",
				flex_wrap = true,
				ui.show(function(): boolean
					local x = e()
					return x ~= nil and #(x.item.threads or {}) > 0 and act.can("open_thread")
				end, function(): Node
					return open_thread_button("secondary")
				end),
				ui.show(function(): boolean
					local x = e()
					return x ~= nil and x.status == "paused" and act.can("resume")
				end, function(): Node
					return Button {
						id = "orch-detail-resume",
						size = "sm",
						tone = "secondary",
						on_click = function()
							local x = entry:peek()
							if x then
								act.resume(x)
							end
						end,
						Icon { name = "action-play", size = 14 },
						locale.t("orch.board.resume"),
					}
				end),
				ui.show(function(): boolean
					local x = e()
					return x ~= nil and x.place == "planned" and x.status ~= "paused" and #(x.item.threads or {}) == 0 and act.can("start")
				end, function(): Node
					return Button {
						id = "orch-detail-start",
						size = "sm",
						tone = "secondary",
						on_click = function()
							local x = entry:peek()
							if x then
								act.start(x)
							end
						end,
						Icon { name = "action-play", size = 14 },
						locale.t("orch.board.start_now"),
					}
				end),
				ui.show(function(): boolean
					local x = e()
					return x ~= nil and Actions.closed(x) and act.can("reopen")
				end, function(): Node
					return Button {
						id = "orch-detail-reopen",
						size = "sm",
						tone = "secondary",
						on_click = function()
							local x = entry:peek()
							if x then
								act.reopen(x)
							end
						end,
						Icon { name = "restart", size = 14 },
						locale.t("orch.board.reopen"),
					}
				end),
			}
		end,
	})

	-- Its facts.
	local status_fact = ui.row {
		id = "orch-detail-status",
		w_full = true,
		min_w = 0,
		min_h = 20,
		gap = "$space.2",
		items = "center",
		Card.Glyph {
			status = function(): string
				local x = e()
				return if x then x.status else "next"
			end,
			size = 14,
		},
		ui.text {
			function(): string
				local x = e()
				return if x then Card.words(x) else ""
			end,
			shrink = 1,
			min_w = 0,
			truncate = true,
			text_size = "$font.size.sm",
			color = "$text.primary",
		},
		ui.text {
			function(): string
				local x = e()
				return if x then Card.tail(ctx, x) else ""
			end,
			shrink = 2,
			min_w = 0,
			truncate = true,
			tabular = true,
			text_size = "$font.size.sm",
			color = "$text.tertiary",
		},
	}
	local threads = ui.computed(function(): { string }
		local x = e()
		return if x then (x.item.threads or {}) else {}
	end)
	local threads_fact = ui.each(threads, function(chat: string): Node
		local function row(): any?
			return ctx.thread(chat)
		end
		return ui.button {
			class = "orch-detail-thread",
			label = function(): string
				local t = row()
				return locale.s("orch.board.open_thread_named", { title = (t and t.title) or chat })
			end,
			w_full = true,
			min_h = 24,
			px = 0,
			gap = "$space.2",
			items = "center",
			rounded = "$radius.sm",
			bg = "#00000000",
			hover = { bg = "$hover" },
			active = { bg = "$pressed" },
			on_click = function()
				local x = entry:peek()
				if x then
					act.open_thread(x, chat)
				end
			end,
			ui.text {
				function(): string
					local t = row()
					return (t and t.title) or chat
				end,
				shrink = 1,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.sm",
				color = "$text.primary",
			},
			ui.text {
				function(): string
					-- (Its state's own name: ds/status.)
					local t = row()
					local k = t and (Status.KINDS :: any)[t.status]
					return if k then k.label else ""
				end,
				shrink = 0,
				text_size = "$font.size.sm",
				color = "$text.tertiary",
			},
			ui.div { grow = 1, min_w = 0 },
			ui.div { shrink = 0, color = "$text.tertiary", Icon { name = "arrow-up-right", size = 12 } },
		}
	end, {
		key = function(chat: string): string
			return chat
		end,
		direction = "column",
		w_full = true,
	})
	-- Its branch and pull request: its finish line's, else its links'.
	local refs = ui.computed(function(): { any }
		local x = e()
		local out: { any } = {}
		if not x then
			return out
		end
		local f = x.card and x.card.finish
		if f and f.branch then
			table.insert(out, { kind = "branch", label = f.branch, target = f.branch })
		end
		for _, l in x.item.links or {} do
			if l.kind == "pr" or (l.kind == "branch" and not (f and f.branch == l.label)) then
				table.insert(out, l)
			end
		end
		return out
	end)
	local refs_fact = ui.each(refs, function(l: any): Node
		return ui.button {
			class = "orch-detail-ref",
			label = l.label,
			min_h = 24,
			px = 0,
			gap = "$space.2",
			items = "center",
			rounded = "$radius.sm",
			bg = "#00000000",
			hover = { bg = "$hover" },
			active = { bg = "$pressed" },
			on_click = function()
				local open = ctx.props.on_link
				if open then
					open(l)
				end
			end,
			ui.div { shrink = 0, color = "$text.tertiary", Icon { name = if l.kind == "pr" then "pull-request" else "git-branch", size = 13 } },
			ui.text { l.label, min_w = 0, truncate = true, text_size = "$font.size.sm", color = "$text.primary" },
		}
	end, {
		key = function(l: any): string
			return `{l.kind}:{l.target}`
		end,
		direction = "column",
		w_full = true,
	})
	local facts = ui.column {
		id = "orch-detail-facts",
		w_full = true,
		min_w = 0,
		gap = "$zeron.space.hair",
		fact("orch.board.fact.status", nil, status_fact),
		fact("orch.board.fact.orchestrator", function(): boolean
			return owner:get() ~= nil
		end, ui.row { min_h = 20, items = "center", min_w = 0, ui.text {
			function(): string
				local o = owner:get()
				return if o then o.name or "" else ""
			end,
			min_w = 0,
			truncate = true,
			text_size = "$font.size.sm",
			color = "$text.primary",
		} }),
		fact("orch.board.fact.threads", function(): boolean
			return #threads:get() > 0
		end, threads_fact),
		fact("orch.board.fact.done_when", function(): boolean
			return card() ~= nil
		end, ui.row { min_h = 20, items = "center", min_w = 0, ui.text {
			function(): string
				return done_when(card())
			end,
			w_full = true,
			wrap = true,
			text_size = "$font.size.sm",
			color = "$text.primary",
		} }),
		fact("orch.board.fact.branch", function(): boolean
			return #refs:get() > 0
		end, refs_fact),
	}

	-- What happened: the engine's lines, oldest first, the last one firmer.
	local events = ui.computed(function(): { any }
		local c = card()
		return (c and c.events) or {}
	end)
	local function count(): number
		return #events:get()
	end
	local timeline = ui.show(function(): boolean
		return count() > 0
	end, ui.column {
		id = "orch-detail-timeline",
		w_full = true,
		min_w = 0,
		gap = "$space.2",
		heading("orch.board.what_happened", count),
		ui.each(events, function(ev: any, i: number): Node
			local function last(): boolean
				return i == count()
			end
			return ui.row {
				class = "orch-detail-event",
				w_full = true,
				min_w = 0,
				gap = "$space.2",
				items = "start",
				-- (Its mark: a small dot, drawn.)
				ui.div {
					shrink = 0,
					w = 12,
					h = (ui.theme.get("font.line.md") :: number?) or 20,
					items = "center",
					justify = "center",
					ui.div {
						w = 5,
						h = 5,
						rounded = "$radius.full",
						bg = function(): string
							return if last() then "$text.secondary" else "$text.tertiary"
						end,
					},
				},
				ui.text {
					ev.text or "",
					grow = 1,
					basis = 0,
					min_w = 0,
					wrap = true,
					selectable = true,
					text_size = "$font.size.sm",
					line_height = "$font.line.md",
					color = function(): string
						return if last() then "$text.primary" else "$text.secondary"
					end,
				},
				ui.text {
					function(): string
						local age = math.max(0, ctx.at() - (ev.at or 0))
						-- (In a panel: the short form, the line's words keeping the room.)
						return if ctx.compact then locale.ago_short(age) else locale.ago(age)
					end,
					shrink = 0,
					tabular = true,
					text_size = "$font.size.xs",
					line_height = "$font.line.md",
					color = "$text.tertiary",
				},
			}
		end, {
			key = function(ev: any): string
				return `{ev.at}:{ev.kind}:{ev.seq or ""}:{ev.about or ""}:{ev.text}`
			end,
			class = "orch-detail-events",
			direction = "column",
			w_full = true,
			p = "$space.2",
			gap = "$space.1",
			rounded = "$radius.lg",
			bg = material.pick("$zeron.settings.note", "$zeron.settings.card"),
		}),
	})

	local flat = props.flat == true
	return ui.column {
		id = `orch-detail-{id}`,
		class = "orch-detail",
		label = locale.t("orch.board.detail"),
		w_full = true,
		min_w = 0,
		p = if flat then nil else "$space.3",
		gap = "$space.3",
		rounded = if flat then nil else "$radius.xl",
		bg = if flat then nil else material.pick("$zeron.settings.tint", "$zeron.settings.group"),
		head,
		ui.column { w_full = true, min_w = 0, gap = "$space.2", title, summary },
		needs_box,
		facts,
		timeline,
	}
end)

-- A card's detail from a host's rows: the whole of it.
M.Card = ui.component(function(props: Ctx.Props): Node
	local ctx = Ctx.make(props)
	local act = Actions.make(ctx)
	local given: any = props.card
	return ui.presence(function(): string?
		return Ctx.read(given)
	end, function(id: string): Node
		local p: any = props
		return M.Detail { ctx = ctx, act = act, card = id, on_close = p.on_close, back = p.back == true, flat = p.flat == true }
	end, { w_full = true })
end)

return M

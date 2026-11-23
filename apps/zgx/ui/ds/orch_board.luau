--!strict
-- The board: where every piece of work stands, in three columns, each with
-- one drawer folded at its foot:
--
--   Needs you   cards with an open ask to the person       Snoozed (asks
--                                                          put off, with
--                                                          when they return)
--   Running     cards an agent is provably working on      Planned (not
--                                                          started, paused,
--                                                          waiting on another)
--   Done        cards that crossed their finish line today Earlier
--
-- Nobody sets a card's column. The engine works each card's place out from
-- facts (its threads, its asks, its finish line, what the person did) and
-- keeps it in the card's row; the board draws it (ds/orch_board_model). So
-- there is nothing to drag: the person answers, snoozes, pauses, marks done
-- or drops, and the card moves when the engine says it has.
--
-- Over the columns: tabs with their counts (All tasks, and each column
-- alone), the orchestrators' filter (the Control room), the switch between
-- the board and its list (ds/orch_board_list: the same places as sections,
-- a line a card), and New task (ds/orch_board_new).
--
-- A click opens a card beside the board (ds/orch_board_detail): its ask's
-- own buttons where it needs the person, its facts, and what happened to
-- it, written by the engine. A secondary click opens its menu. What the
-- person does is said in a toast when it took; a move the engine made
-- shows a dot on the card instead, until it's opened.
--
-- Shared by the Control room (every orchestrator's, `show_filter`) and an
-- orchestrator's page (its own, `filter`, `compact`): one component. Its
-- props are ds/orch_board_ctx's.
--
--   OrchBoard {
--     items = fn, cards = fn, asks = fn, orchestrators = fn,
--     thread_of = fn(chat), progress_of = fn(chat),
--     show_filter = true,
--     on_open_thread = fn(chat), on_answer = fn(ask, choice), on_snooze = fn(ask_id, until),
--     on_close = fn(id, dropped), on_pause = fn(id), on_resume = fn(id), …
--   }

local Icon = require("@self/icon")
local ContextMenu = require("@self/context_menu")
local Tabs = require("@self/tabs")
local Button = require("@self/button")
local IconButton = require("@self/icon_button").IconButton
local hue = require("@self/orch_hue")
local locale = require("@self/locale")
local material = require("@self/material")
local toast = require("@self/toast")
local model = require("@self/orch_board_model")
local Ctx = require("@self/orch_board_ctx")
local Card = require("@self/orch_board_card")
local Detail = require("@self/orch_board_detail")
local List = require("@self/orch_board_list")
local NewTask = require("@self/orch_board_new")
local Actions = require("@self/orch_board_actions")
local trace = require("@self/orch_perf_trace")

export type Props = Ctx.Props

-- A card shown from elsewhere (the Map's hover card, its Open card), by id.
local reveal = ui.shared("control_board_reveal", nil :: string?)

-- A column's cards before "Show n more"; a drawer's, and how many more
-- each press shows.
local CAP = 12
local DRAWER_CAP = 20
-- A column's narrowest: under three of them side by side, they stack.
local COLUMN_MIN = 240
-- The detail's width beside the board.
local DETAIL_W = 420

-- Each column's glyph's status (its ink and shape are the status').
local HEAD: { [string]: string } = { needs_you = "asked", running = "working", done = "done" }

local OrchBoard = ui.component(function(props: Props): Node
	local ctx = Ctx.make(props)
	local act = Actions.make(ctx)
	local compact = ctx.compact
	local view = Ctx.kept(props.view, ui.use_state("board"))
	local tab = Ctx.kept(props.tab, ui.use_state("all"))
	local drawers = Ctx.kept(props.drawers, ui.use_state({} :: { [string]: boolean }))
	local new_open = ui.use_state(false)
	-- How many of each place show (past its cap).
	local shown = ui.use_state({} :: { [string]: number })

	-- A place's cards' ids, in order (woken when who is there changes).
	local function ids_of(place: string): { string }
		return ctx.ids(place)
	end
	local function cap_of(place: string): number
		return shown:get()[place] or (if model.COLUMN[place] then DRAWER_CAP else CAP)
	end

	-- A card's menu: what the person can do with it, as it stands.
	local function menu(e: Ctx.Entry): { any }?
		return act.menu(e)
	end
	-- A paused card's Resume, under its foot.
	local function resume_row(entry: () -> Ctx.Entry?): Node?
		if not props.on_resume then
			return nil
		end
		return ui.show(function(): boolean
			local e = entry()
			return e ~= nil and e.status == "paused"
		end, function(): Node
			return ui.row {
				class = "orch-card-resume-row",
				w_full = true,
				pt = "$zeron.space.hair",
				Button {
					class = "orch-card-resume",
					size = "xs",
					tone = "secondary",
					on_click = function()
						local e = entry()
						if e then
							act.resume(e)
						end
					end,
					Icon { name = "action-play", size = 12 },
					locale.t("orch.board.resume"),
				},
			}
		end)
	end

	-- A place's cards, as many as show, then "Show n more".
	local function cards(place: string, drawer: boolean): Node
		local ids = ui.computed(function(): { string }
			trace.hit("board.ids")
			local all = ids_of(place)
			local cap = cap_of(place)
			if #all <= cap then
				return all
			end
			local out: { string } = {}
			for i = 1, math.min(cap, #all) do
				table.insert(out, all[i])
			end
			return out
		end)
		local more = ui.computed(function(): number
			return math.max(0, #ids_of(place) - cap_of(place))
		end)
		return ui.column {
			w_full = true,
			min_w = 0,
			gap = "$space.1",
			ui.each(ids, function(id: string): Node
				return Card.Card { ctx = ctx, card = id, drawer = drawer, menu = menu, extra = resume_row }
			end, {
				key = function(id: string): string
					return id
				end,
				class = "orch-cards",
				direction = "column",
				w_full = true,
				gap = "$space.1",
			}),
			ui.show(function(): boolean
				return more:get() > 0
			end, function(): Node
				return ui.button {
					class = "orch-show-more",
					w_full = true,
					h = 26,
					items = "center",
					justify = "center",
					rounded = "$radius.md",
					bg = "#00000000",
					hover = { bg = "$hover" },
					active = { bg = "$pressed" },
					color = "$text.tertiary",
					on_click = function()
						local step = if drawer then DRAWER_CAP else CAP
						shown:set_key(place, cap_of(place) + step)
					end,
					ui.text {
						function(): string
							return locale.s("orch.board.show_more", { n = more:get() })
						end,
						text_size = "$font.size.xs",
						weight = "$font.weight.medium",
					},
				}
			end),
		}
	end

	-- The drawer at a column's foot: folded, a line saying how many and
	-- what's first; open, its cards.
	local function drawer(place: string): Node
		local function open(): boolean
			return drawers:get()[place] == true
		end
		local function count(): number
			return #ids_of(place)
		end
		-- (Its first card, read by itself.)
		local function first(): Ctx.Entry?
			return ctx.entry(ids_of(place)[1])
		end
		local hint = ui.computed(function(): string
			if place == "planned" then
				local e = first()
				local next = e and (e.status == "next" or e.status == "queued")
				return if next then locale.s("orch.board.next_named", { title = (e :: Ctx.Entry).item.title or "" }) else ""
			elseif place == "snoozed" then
				local e = first()
				return if e then e.item.title or "" else ""
			end
			return locale.s("orch.board.before_today")
		end)
		local back = ui.computed(function(): string
			if place ~= "snoozed" then
				return ""
			end
			local e = first()
			return if e and e.until_at then Card.time(e.until_at, ctx.clock(0)) else ""
		end)
		return ui.show(function(): boolean
			return count() > 0
		end, ui.column {
			id = `orch-drawer-{place}`,
			class = "orch-drawer",
			w_full = true,
			min_w = 0,
			gap = "$space.1",
			ui.button {
				class = "orch-drawer-head",
				label = locale.t(`orch.board.place.{place}`),
				expanded = open,
				w_full = true,
				h = 32,
				px = "$space.2",
				gap = "$space.2",
				items = "center",
				rounded = "$radius.md",
				border = 1,
				border_color = "$border",
				border_dash = { 3, 3 },
				bg = "#00000000",
				hover = { bg = "$hover" },
				active = { bg = "$pressed" },
				on_click = function()
					drawers:set_key(place, not open())
				end,
				ui.div {
					shrink = 0,
					color = "$text.tertiary",
					Icon {
						name = function(): Icon.Name
							return if open() then "alt-arrow-down" else "alt-arrow-right"
						end,
						size = 13,
					},
				},
				ui.text {
					locale.t(`orch.board.place.{place}`),
					class = "orch-drawer-title",
					shrink = 0,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					color = "$text.secondary",
				},
				ui.text {
					function(): string
						return tostring(count())
					end,
					class = "orch-drawer-count",
					shrink = 0,
					tabular = true,
					text_size = "$font.size.sm",
					color = "$text.tertiary",
				},
				ui.div { grow = 1, min_w = 0 },
				ui.show(function(): boolean
					return not open() and hint:get() ~= ""
				end, ui.text {
					function(): string
						return hint:get()
					end,
					class = "orch-drawer-hint",
					shrink = 1,
					min_w = 0,
					truncate = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				}),
				ui.show(function(): boolean
					return not open() and back:get() ~= ""
				end, ui.row {
					class = "orch-drawer-back",
					shrink = 0,
					gap = "$zeron.space.tight",
					items = "center",
					color = "$text.tertiary",
					Icon { name = "clock-circle", size = 12 },
					ui.text {
						function(): string
							return back:get()
						end,
						tabular = true,
						text_size = "$font.size.xs",
					},
				}),
			},
			ui.show(open, function(): Node
				return cards(place, true)
			end),
		})
	end

	-- A column: its head (its glyph, name and count), its cards, its drawer.
	local function column(place: string): Node
		local lane = material.pick("$zeron.settings.tint", "$zeron.settings.group")
		local function count(): number
			return #ids_of(place)
		end
		return ui.column {
			id = `orch-col-{place}`,
			class = "orch-column",
			label = locale.t(`orch.board.place.{place}`),
			grow = 1,
			basis = 0,
			min_w = 0,
			p = "$space.1",
			gap = "$space.1",
			rounded = "$radius.xl",
			bg = lane,
			hidden = function(): boolean
				local t = tab:get()
				return t ~= "all" and t ~= place
			end,
			ui.row {
				class = "orch-column-head",
				w_full = true,
				h = 30,
				px = "$space.2",
				gap = "$space.2",
				items = "center",
				Card.Glyph {
					status = function(): string
						return HEAD[place]
					end,
					size = 14,
				},
				ui.text {
					locale.t(`orch.board.place.{place}`),
					class = "orch-column-title",
					text_size = "$font.size.sm",
					weight = "$font.weight.semibold",
					color = "$text.primary",
				},
				ui.div { grow = 1, min_w = 0 },
				ui.text {
					function(): string
						return tostring(count())
					end,
					class = "orch-column-count",
					tabular = true,
					text_size = "$font.size.sm",
					color = "$text.tertiary",
				},
			},
			cards(place, false),
			ui.show(function(): boolean
				return count() == 0
			end, ui.row {
				class = "orch-column-empty",
				w_full = true,
				h = 44,
				items = "center",
				justify = "center",
				ui.text {
					locale.t(`orch.board.none.{place}`),
					text_size = "$font.size.sm",
					color = "$text.tertiary",
				},
			}),
			drawer(model.DRAWER[place]),
		}
	end

	-- The columns: side by side where three fit, else one under another.
	local lanes = ui.ref()
	local stacked = ui.computed(function(): boolean
		if compact then
			return true
		end
		local w = lanes.width
		return w ~= nil and w > 0 and w < COLUMN_MIN * 3 + 16
	end)
	local function columns(): Node
		return ui.row {
			ref = lanes,
			id = "orch-columns",
			class = "orch-columns",
			w_full = true,
			min_w = 0,
			gap = "$space.2",
			-- (Stacked, each column takes the board's width.)
			items = function(): "start" | "stretch"
				return if stacked:get() then "stretch" else "start"
			end,
			direction = function(): "row" | "column"
				return if stacked:get() then "column" else "row"
			end,
			column("needs_you"),
			column("running"),
			column("done"),
		}
	end

	-- The orchestrators' filter: a button saying whose cards show, its menu
	-- every orchestrator's name.
	local filter_open = ui.use_state(false)
	local function filter_button(): Node
		local function chosen(): any?
			return ctx.owner(ctx.filter())
		end
		return (ContextMenu {
			open = filter_open,
			label = locale.t("orch.board.filter"),
			shrink = 0,
			items = function(): { any }?
				local entries: { any } = {
					{
						label = locale.s("orch.board.all_orchestrators"),
						checked = function(): boolean
							return ctx.filter() == nil
						end,
						on_select = function()
							ctx.set_filter(nil)
						end,
					},
					{ separator = true },
				}
				for _, o in ctx.orchestrators() do
					if not o.archived and o.kind ~= "brain_dump" then
						table.insert(entries, {
							label = o.name,
							checked = function(): boolean
								return ctx.filter() == o.id
							end,
							on_select = function()
								ctx.set_filter(o.id)
							end,
						})
					end
				end
				return entries
			end,
			ui.button {
				id = "orch-board-filter",
				class = "orch-board-filter",
				label = locale.t("orch.board.filter"),
				h = 28,
				px = "$space.2",
				gap = "$space.2",
				items = "center",
				rounded = "$radius.md",
				bg = "#00000000",
				hover = { bg = "$hover" },
				active = { bg = "$pressed" },
				color = "$text.secondary",
				on_click = function()
					filter_open:set(true)
				end,
				ui.show(function(): boolean
					return chosen() == nil
				end, Icon { name = "tuning", size = 14 }, hue.Dot {
					hue = function(): number?
						local o = chosen()
						return o and o.hue
					end,
					size = 7,
				}),
				ui.text {
					function(): string
						local o = chosen()
						return if o then o.name else locale.s("orch.board.filter")
					end,
					max_w = 180,
					truncate = true,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
				},
				ui.div { color = "$text.tertiary", Icon { name = "alt-arrow-down", size = 12 } },
			},
		}) :: Node
	end

	-- The board or its list: two glyphs, the one showing held.
	local function view_switch(): Node
		local function one(name: string, icon: Icon.Name, key: string): Node
			return IconButton {
				id = `orch-board-view-{name}`,
				icon = icon,
				label = locale.t(key),
				size = "lg",
				tone = "quiet",
				lit = function(): boolean
					return view:get() == name
				end,
				on_click = function()
					view:set(name)
				end,
			}
		end
		return ui.row {
			class = "orch-board-view",
			shrink = 0,
			gap = "$zeron.space.hair",
			items = "center",
			one("board", "split-columns", "orch.board.view.board"),
			one("list", "list", "orch.board.view.list"),
		}
	end

	local function count_of(key: string): () -> number
		return function(): number
			trace.hit("board.counts")
			return (ctx.counts() :: any)[key] or 0
		end
	end
	local head = ui.row {
		id = "orch-board-head",
		class = "orch-board-head",
		w_full = true,
		min_w = 0,
		gap = "$space.2",
		items = "center",
		flex_wrap = true,
		Tabs {
			id = "orch-board-tabs",
			value = tab,
			look = "pill",
			size = "sm",
			label = locale.t("orch.board.tabs"),
			options = {
				{ value = "all", label = locale.t("orch.board.tab.all"), count = count_of("all") },
				{ value = "needs_you", label = locale.t("orch.board.place.needs_you"), count = count_of("needs_you") },
				{ value = "running", label = locale.t("orch.board.place.running"), count = count_of("running") },
				{ value = "done", label = locale.t("orch.board.place.done"), count = count_of("done") },
			},
		},
		ui.div { grow = 1, min_w = 0 },
		if props.show_filter then filter_button() else nil,
		view_switch(),
		if props.on_new_task
			then Button {
				id = "orch-board-new",
				size = "sm",
				tone = "primary",
				on_click = function()
					new_open:set(true)
				end,
				Icon { name = "plus", size = 14 },
				locale.t("orch.board.new_task"),
			}
			else nil,
	}

	-- A card shown from elsewhere: opened here.
	ui.effect(function()
		local id = reveal:get()
		if id and ctx.entry(id) then
			ctx.open(id)
		end
	end)
	-- The open card gone from the board (taken off it): its detail closes.
	ui.effect(function()
		local id = ctx.selected()
		if id and not ctx.entry(id) then
			ctx.open(nil)
		end
	end)

	local function main(): Node
		return ui.switch(function(): string
			return view:get()
		end, {
			board = function(): Node
				return columns()
			end,
			list = function(): Node
				return List.Sections { ctx = ctx, act = act, tab = tab }
			end,
		})
	end

	return ui.column {
		id = props.id or "orch-board",
		class = "orch-board",
		label = locale.t("orch.board.label"),
		w_full = true,
		min_w = 0,
		gap = "$space.3",
		head,
		ui.row {
			w_full = true,
			min_w = 0,
			gap = "$space.3",
			items = "start",
			ui.column { grow = 1, basis = 0, min_w = 0, main() },
			-- Its detail, beside the board (a host that shows details
			-- itself, `on_open_card`, has none here).
			if props.on_open_card
				then nil
				else ui.presence(function(): string?
					return ctx.selected()
				end, function(id: string): Node
					return ui.column {
						id = "orch-board-detail",
						class = "orch-board-detail",
						w = if compact then nil else DETAIL_W,
						w_full = compact,
						shrink = 0,
						Detail.Detail {
							ctx = ctx,
							act = act,
							card = id,
							on_close = function()
								ctx.open(nil)
							end,
						},
					}
				end),
		},
		NewTask.Dialog {
			open = new_open,
			orchestrators = ctx.orchestrators,
			chosen = ctx.filter,
			on_send = function(text: string, orchestrator_id: string?): any
				local send = props.on_new_task
				if not send then
					return nil
				end
				local sent = send(text, orchestrator_id)
				Ctx.after(sent, function(err: any)
					toast.show { title = locale.s("orch.board.new_task_failed"), body = act.reason(err), tone = "danger" }
				end, function()
					toast.show { title = locale.s("orch.board.new_task_sent"), body = text }
				end)
				return sent
			end,
		},
	}
end)

return OrchBoard

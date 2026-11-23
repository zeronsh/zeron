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
local reduce = require("@self/motion")
local Motion = require("@self/orch_board_motion")

export type Props = Ctx.Props

-- A card shown from elsewhere (the Map's hover card, its Open card), by id.
local reveal = ui.shared("control_board_reveal", nil :: string?)

-- A column's cards before "Show n more"; a drawer's, and how many more
-- each press shows.
local CAP = 12
local DRAWER_CAP = 20
-- A column's narrowest: under three of them side by side, they stack.
local COLUMN_MIN = 240
-- A folded drawer's head line and the column's inset under it: where a
-- card going into the drawer, or coming out, is cut.
local HEAD_LINE = Motion.HEAD_LINE
-- How far the copies' layer reaches under the column, clipped.
local GHOST_EXTRA = 600
-- How far a lane's body reaches under its foot, clipped: as far as a
-- column can shrink at once.
local LANE_EXTRA = 4000
-- A number from what anime answers (a number, or a string with one).
local function num_of(v: any, default: number): number
	if type(v) == "number" then
		return v
	end
	return tonumber(string.match(tostring(v or ""), "-?[%d%.]+") or "") or default
end
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
	-- (A drawer folded on the board: a card goes into it, or comes out of
	-- it, by its head line.)
	ctx.motion.set_folded(function(place: string): boolean
		return view:peek() == "board" and model.COLUMN[place] ~= nil and drawers:peek()[place] ~= true
	end)

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
			-- (A card travelling in goes over its column's drawer too.)
			z = function(): number
				return if ctx.motion.raised(model.COLUMN[place] or place) then 1 else 0
			end,
			ui.each(ids, function(id: string): Node
				return Card.Card { ctx = ctx, card = id, drawer = drawer, menu = menu, extra = resume_row }
			end, {
				key = function(id: string): string
					return id
				end,
				-- (The gap a card leaves closes, the slot it lands in opens,
				-- and a card that changes its order slides.)
				animate = reduce.layout({ duration = Motion.TRAVEL, ease = Motion.EASE }),
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

	-- Each drawer's node, for its column to ease it with its lane.
	local drawer_refs: { [string]: any } = {}
	for _, place in { "snoozed", "planned", "earlier" } do
		drawer_refs[place] = ui.ref()
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
		-- (A node of its own in the column, hidden while empty: as cards
		-- move above it, the column eases it with its lane (`column`).)
		return ui.div {
			ref = drawer_refs[place],
			hidden = function(): boolean
				return count() == 0
			end,
			w_full = true,
			min_w = 0,
			ui.show(function(): boolean
				return count() > 0
			end, ui.column {
				id = `orch-drawer-{place}`,
				class = "orch-drawer",
				w_full = true,
				min_w = 0,
				ui.button {
					class = "orch-drawer-head",
					on_mount = function(el: any)
						ctx.motion.head(place, el)
					end,
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
				-- Its cards, the space they take easing as it opens and closes
				-- (clipped: what goes fades under its edge).
				ui.div {
					class = "orch-drawer-body",
					w_full = true,
					min_w = 0,
					layout = if reduce.reduced_now() then nil else "height",
					layout_duration = Motion.DRAWER,
					layout_ease = Motion.EASE,
					layout_clip = true,
					ui.presence(function(): string?
						return if open() then "open" else nil
					end, function(): Node
						return ui.column {
							w_full = true,
							min_w = 0,
							pt = "$space.1",
							cards(place, true),
						}
					end, {
						w_full = true,
						exit = reduce.transition({ opacity = 0, duration = Motion.DRAWER, ease = Motion.EASE }),
					}),
				},
			}),
		}
	end

	-- A column: its head (its glyph, name and count), its cards, its drawer.
	local function column(place: string): Node
		local lane = material.pick("$zeron.settings.tint", "$zeron.settings.group")
		local function count(): number
			return #ids_of(place)
		end
		-- Its lane: a body and a rounded foot drawn under it, so that as the
		-- column grows or shrinks the lane eases with its cards on the
		-- compositor (the body's clip and the foot's translation, the cards'
		-- curve): no card or drawer hangs outside it as it shrinks.
		local radius = (ui.theme.get("radius.xl") :: number?) or 12
		local body, foot, ghost_layer = ui.ref(), ui.ref(), ui.ref()
		local last_h: number? = nil
		-- Only a card move (or a drawer opening and closing, whose space
		-- eases by itself) moves the lane on a curve: at mount, on a tab
		-- or filter, a new width (the window, the detail), everything in
		-- the column takes its place at once, together.
		local function eased(size: { w: number, h: number })
			local was = last_h
			last_h = size.h
			if was == nil or was <= 0 or size.h <= 0 or reduce.reduced_now() or not ctx.motion.recent(place) then
				return
			end
			-- (How much taller it showed: what the lane eases away.)
			local d = was - size.h
			if math.abs(d) < 0.5 then
				return
			end
			local b, f = body:get(), foot:get()
			if not b or not f then
				return
			end
			local from = num_of(anime.utils.get(b, "clipBottom"), LANE_EXTRA) - d
			local from_y = d + num_of(anime.utils.get(f, "y"), 0)
			anime.utils.set(b, { clipBottom = from })
			anime.utils.set(f, { y = from_y })
			anime.animate(b, { clipBottom = { to = LANE_EXTRA }, duration = Motion.TRAVEL, ease = Motion.EASE })
			anime.animate(f, { y = { to = 0 }, duration = Motion.TRAVEL, ease = Motion.EASE })
			-- (Its drawer, the column's last, moves as its foot does.)
			local dr = drawer_refs[model.DRAWER[place]]
			local de = dr and dr:get()
			if de then
				anime.utils.set(de, { y = d + num_of(anime.utils.get(de, "y"), 0) })
				anime.animate(de, { y = { to = 0 }, duration = Motion.TRAVEL, ease = Motion.EASE })
			end
			-- (The copies' cut, the head line's top, with it.)
			local g = ghost_layer:get()
			if g then
				local base = GHOST_EXTRA + HEAD_LINE
				anime.utils.set(g, { clipBottom = num_of(anime.utils.get(g, "clipBottom"), base) - d })
				anime.animate(g, { clipBottom = { to = base }, duration = Motion.TRAVEL, ease = Motion.EASE })
			end
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
			relative = true,
			on_resize = eased,
			on_mount = function(el: any)
				ctx.motion.column(place, el)
			end,
			ui.div {
				ref = body,
				class = "orch-lane-body",
				-- (Drawn only: the pointer passes through to what is under it.)
				pointer_events = "none",
				absolute = true,
				top = 0,
				left = 0,
				right = 0,
				bottom = radius - LANE_EXTRA,
				rounded = "$radius.xl",
				rounded_bl = 0,
				rounded_br = 0,
				bg = lane,
				clip_inset = { top = 0, right = 0, bottom = LANE_EXTRA, left = 0 },
				-- (Its clip given as an animated value too, from its first
				-- frame: the one its eases start from.)
				on_mount = function(el: any)
					anime.utils.set(el, { clipBottom = LANE_EXTRA })
				end,
			},
			ui.div {
				ref = foot,
				class = "orch-lane-foot",
				-- (Drawn only: the pointer passes through to what is under it.)
				pointer_events = "none",
				absolute = true,
				left = 0,
				right = 0,
				bottom = 0,
				h = ui.raw(radius, "a lane's rounded foot: its corner's height"),
				rounded = "$radius.xl",
				rounded_tl = 0,
				rounded_tr = 0,
				bg = lane,
			},
			-- (A card travelling into it goes over the other columns.)
			z = function(): number
				return if ctx.motion.raised(place) then 1 else 0
			end,
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
			-- (Once the last card has left, not under it: then in.)
			-- (Its room taken as the last card leaves, so the drawer settles
			-- once; its words come in once that card has gone.)
			ui.show(function(): boolean
				return count() == 0
			end, function(): Node
				return ui.row {
					class = "orch-column-empty",
					w_full = true,
					h = 44,
					items = "center",
					justify = "center",
					ui.text {
						locale.t(`orch.board.none.{place}`),
						class = "orch-column-empty-words",
						opacity = reduce.tween(
							ui.computed(function(): number
								return if ctx.motion.leaving(place) then 0 else 1
							end),
							{ duration = Motion.FADE, ease = "linear" }
						),
						text_size = "$font.size.sm",
						color = "$text.tertiary",
					},
				}
			end),
			-- Over its cards, under its drawer: the copies of cards going into
			-- its folded drawer, each from where its card stood in under the
			-- drawer's head line.
			ui.div {
				class = "orch-ghosts",
				ref = ghost_layer,
				-- (Drawn only: the pointer passes through to what is under it.)
				pointer_events = "none",
				absolute = true,
				top = 0,
				left = 0,
				right = 0,
				-- Over the column's cards (a copy goes over those closing its
				-- gap), and cut at the top edge of the drawer's head line (its
				-- 32 points and the column's inset above the column's foot,
				-- following the drawer as it eases): a copy slides in behind
				-- the line, or rises out from behind it, never over its words
				-- nor below the lane.
				z = 3,
				bottom = -GHOST_EXTRA,
				clip_inset = { top = 0, right = 0, bottom = GHOST_EXTRA + HEAD_LINE, left = 0 },
				on_mount = function(el: any)
					anime.utils.set(el, { clipBottom = GHOST_EXTRA + HEAD_LINE })
				end,
				ui.each(
					ui.computed(function(): { any }
						return ctx.motion.ghosts(place)
					end),
					function(g: any): Node
						return ui.div {
							absolute = true,
							left = g.x,
							top = g.y,
							w = g.w,
							on_mount = function(el: any)
								ctx.motion.ghost_placed(g, el)
							end,
							Card.Card { ctx = ctx, card = g.id, ghost = true },
						}
					end,
					{
						key = function(g: any): string
							return g.key
						end,
					}
				),
			},
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
	local function lanes_row(): Node
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
	-- (Built again for each filter: a filter is no move, so nothing slides.)
	local function columns(): Node
		return ui.presence(function(): string
			return ctx.filter() or "*"
		end, function(): Node
			return lanes_row()
		end, { w_full = true, min_w = 0 })
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
				local shown: { any } = {}
				local named: { [string]: number } = {}
				for _, o in ctx.orchestrators() do
					if not o.archived and o.kind ~= "brain_dump" then
						table.insert(shown, o)
						named[o.name] = (named[o.name] or 0) + 1
					end
				end
				-- (Two of one name: each says its project under it.)
				local project_of: ((string) -> string?)? = (props :: any).project_of
				for _, o in shown do
					do
						local project: string? = nil
						if named[o.name] > 1 and project_of then
							project = project_of(o.id)
						end
						table.insert(entries, {
							label = o.name,
							lead = function(): Node
								return hue.Dot { hue = o.hue, size = 7 }
							end,
							detail = if project and project ~= "" then { icon = "folder", text = project } else nil,
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

	-- Whether a card's detail shows here.
	local function detail_shown(): boolean
		return props.on_open_card == nil and ctx.selected() ~= nil
	end

	-- The board to its list and back: the leaving view fades where it
	-- stands, and the cards (or rows) in view slide in from where their
	-- counterparts stood (ds/orch_board_motion), told as the switch picks its
	-- side, before either is built.
	local last_view: string? = nil
	-- The open card's detail: in with a fade and a slide from the right,
	-- the columns taking their new width over the same while, and out
	-- quicker (ds/orch_board_motion). One card to another swaps in place.
	local function detail(): Node
		local reduced = reduce.reduced_now()
		local function one(id: string): Node
			return Detail.Detail {
				ctx = ctx,
				act = act,
				card = id,
				back = compact,
				on_back = function()
					ctx.open(nil)
				end,
			}
		end
		local last_open: string? = nil
		local function opened(): Node
			return ui.column {
				id = "orch-board-detail",
				class = "orch-board-detail",
				w = if compact then nil else DETAIL_W,
				w_full = compact,
				ml = if compact then nil else "$space.3",
				shrink = 0,
				-- (The card last open while it closes: what fades is its
				-- detail, not an empty box.)
				ui.presence(function(): string?
					local id = ctx.selected()
					if id ~= nil then
						last_open = id
					end
					return last_open
				end, one, { w_full = true }),
			}
		end
		-- Its content fades in (180 ms) and out (130 ms), laid out once at
		-- its full width; beside the board, its box eases its width over
		-- 180 ms both ways with the content pinned to its right edge and
		-- cut by it, so the columns widen and narrow with it. Closing, the
		-- content stays where it stands (in the box's flow, so still
		-- pinned right) until the box has closed.
		local closing = ui.use_state(false)
		local content = ui.ref()
		local shown = ui.presence(function(): string?
			return if ctx.selected() ~= nil or closing:get() then "open" else nil
		end, function(): Node
			local node = opened()
			return ui.div { ref = content, shrink = 0, w_full = compact, node }
		end, {
			enter = { opacity = { 0, 1 }, duration = if reduced then Motion.FADE else Motion.DETAIL_IN, ease = "linear" },
		})
		if compact then
			return shown
		end
		local full = DETAIL_W + ((ui.theme.get("space.3") :: number?) or 12)
		local box = ui.ref()
		local was_open = ctx.selected() ~= nil
		ui.effect(function()
			local open = ctx.selected() ~= nil
			if open == was_open then
				return
			end
			was_open = open
			local now_reduced = reduce.reduced_now()
			local c = content:get()
			if open then
				closing:set(false)
				if c then
					anime.animate(c, { opacity = { to = 1 }, duration = if now_reduced then Motion.FADE else Motion.DETAIL_IN, ease = "linear" })
				end
			else
				closing:set(true)
				if c then
					anime.animate(c, { opacity = { to = 0 }, duration = if now_reduced then Motion.FADE else Motion.DETAIL_OUT, ease = "linear" })
				end
			end
			local el = box:get()
			if el then
				anime.animate(el, {
					w = if open then full else 0,
					duration = if now_reduced then Motion.FADE else Motion.DETAIL_IN,
					ease = Motion.EASE,
					onComplete = function()
						if not open then
							closing:set(false)
						end
					end,
				})
			else
				closing:set(false)
			end
		end)
		return ui.row {
			ref = box,
			class = "orch-board-detail-space",
			shrink = 0,
			self = "stretch",
			w = if was_open then full else 0,
			justify = "end",
			items = "start",
			overflow = "hidden",
			shown,
		}
	end

	local function main(): Node
		local reduced = reduce.reduced_now()
		return ui.presence(function(): string
			local v = view:get()
			if last_view ~= nil and v ~= last_view then
				ctx.motion.begin_switch()
			end
			last_view = v
			return v
		end, function(v: string): Node
			if v == "list" then
				return List.Sections { ctx = ctx, act = act, tab = tab }
			end
			return columns()
		end, {
			w_full = true,
			min_w = 0,
			-- (What leaves fades where it stands; what comes, its cards and rows
			-- aside, fades in where it lands.)
			exit = { opacity = 0, duration = if reduced then Motion.FADE else Motion.SWITCH_OUT, ease = "linear" },
			-- (From a hundredth, not nothing: a list counts what is wholly
			-- transparent out of view, and would build its rows a frame late.)
			enter = {
				opacity = { 0.01, 1 },
				duration = if reduced then Motion.FADE else Motion.SWITCH_IN,
				delay = if reduced then 0 else Motion.SWITCH_DELAY,
				ease = "linear",
			},
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
			items = "start",
			-- Compact (a narrow page) with a card open: its detail in the
			-- board's place, Back to the board as it was (kept mounted,
			-- hidden; where the page is scrolled is the host's to keep, as
			-- an orchestrator's side panel does).
			ui.column {
				grow = 1,
				basis = 0,
				min_w = 0,
				hidden = function(): boolean
					return compact and detail_shown()
				end,
				main(),
			},
			-- Its detail, beside the board (a host that shows details
			-- itself, `on_open_card`, has none here). The gap is the
			-- detail's own: with none open, the columns reach the edge.
			if props.on_open_card then nil else detail(),
		},
		-- (The timers of what moves: ds/orch_board_motion.)
		ctx.motion.host(),
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
					toast.show { title = locale.s("orch.board.new_task_sent"), body = text, tone = "success" }
				end)
				return sent
			end,
		},
	}
end)

return OrchBoard

--!strict
-- The board as a list: the same places as folding sections (Needs you,
-- Running, Planned, Done), a line a card. A line is the card's status
-- glyph, whose it is, its title, then its status' words and clock, its
-- threads and its one chip; a card that needs the person carries its
-- question and the ask's own buttons under its line. Snoozed folds under
-- Needs you and Earlier under Done, as the board's drawers do. A click
-- opens the card (its detail); a secondary click, its menu.
--
-- Two ways in. Inside the board (its list view), `Sections` shares the
-- board's own rows. By itself (an orchestrator's side panel), `List` takes
-- the board's props (ds/orch_board_ctx) and is the whole of it, at any
-- width:
--
--   List.List {
--     items = fn, cards = fn, asks = fn, orchestrators = fn,
--     thread_of = fn(chat), progress_of = fn(chat),
--     filter = fn -> orchestrator id,       -- its cards alone
--     compact = true,                        -- a side panel's width
--     fold = { open = fn(key, default) -> boolean, set = fn(key, open) },
--                                            -- where sections are folded,
--                                            -- kept by the host (keys:
--                                            -- needs_you, snoozed, running,
--                                            -- planned, done, earlier)
--     on_open_card = fn(id),                 -- a card opened: the host
--                                            -- shows its detail
--     on_answer = fn(ask, choice), on_snooze = fn(ask_id, until_ms?), …
--   }

local Icon = require("@self/icon")
local Button = require("@self/button")
local ContextMenu = require("@self/context_menu")
local Choices = require("@self/orch_choices")
local Fold = require("@self/orch_fold")
local hue = require("@self/orch_hue")
local locale = require("@self/locale")
local Ctx = require("@self/orch_board_ctx")
local Actions = require("@self/orch_board_actions")
local Card = require("@self/orch_board_card")
local trace = require("@self/orch_perf_trace")
local reduce = require("@self/motion")
local Motion = require("@self/orch_board_motion")

local M = {}

export type FoldState = { open: (key: string, default: boolean) -> boolean, set: (key: string, open: boolean) -> () }

-- The sections, in order, and what folds under two of them.
M.SECTIONS = { "needs_you", "running", "planned", "done" }
local UNDER: { [string]: string } = { needs_you = "snoozed", done = "earlier" }
-- Which sections a tab of the board shows.
local TAB: { [string]: { [string]: boolean } } = {
	needs_you = { needs_you = true },
	running = { running = true, planned = true },
	done = { done = true },
}
-- Open until folded: the sections; folded until opened: what's under them.
local OPEN: { [string]: boolean } = { needs_you = true, running = true, planned = true, done = true, snoozed = false, earlier = false }
-- A section's rows before "Show n more".
local CAP = 30
-- Under this width a row's status words give way to its clock.
local NARROW = 440
local HEAD: { [string]: string } = { needs_you = "asked", running = "working", planned = "next", done = "done" }

-- Only the rows near the view are built (in whatever scrolls the list: a
-- page, a side panel): each row stands in a slot that hears from zgx
-- whether it is within MARGIN of the view (`on_visible`, its box grown by
-- `visible_margin`), as the transcript's runs do (ds/transcript). Away from
-- it, a slot holds the height its row was last laid out at (or a guess),
-- so the page keeps its length and nothing in view moves. When the list
-- first shows, the rows a view's height down are built at once (no frame
-- of empty slots); rows that come later (a section opened, Show more, a
-- card arriving) are built, and let go at their first layout if out of
-- reach.
local MARGIN = 240
-- A row's guessed height: its line, and an ask's question and buttons, or
-- Resume, under it (measured: 30, 94, 62).
local LINE_H, ASK_H, RESUME_H = 30, 94, 62
-- The heights rows were laid out at, by card (and the list's density).
local HEIGHTS: { [string]: number } = {}
-- How far down a list builds its rows at once, past the window's height.
local function first_reach(): number
	local window = (ui :: any).window
	local ok, size = pcall(function(): any
		return window.size:peek()
	end)
	local h = if ok and type(size) == "table" and type(size.height) == "number" then size.height else 0
	if h < 200 then
		h = 1200
	end
	return h + MARGIN
end

type RowProps = { ctx: Ctx.Ctx, act: Actions.Actions, card: string, narrow: () -> boolean }

local Row = ui.component(function(props: RowProps): Node
	local ctx, act, id = props.ctx, props.act, props.card
	trace.hit("board.row")
	local entry = ui.computed(function(): Ctx.Entry?
		trace.hit("board.row_entry")
		return ctx.entry(id)
	end)
	local function e(): Ctx.Entry?
		return entry:get()
	end
	local selected = ui.computed(function(): boolean
		return ctx.selected() == id
	end)
	local tail = ui.computed(function(): string
		local x = e()
		return if x then Card.tail(ctx, x) else ""
	end)
	-- (Where the list is narrow the title keeps the room: its status' words
	-- give way to its clock where it has one, the glyph still saying which.)
	local words = ui.computed(function(): string
		local x = e()
		if not x or (props.narrow() and tail:get() ~= "") then
			return ""
		end
		return Card.words(x)
	end)
	local ask = ui.computed(function(): any?
		local x = e()
		return if x and x.place == "needs_you" then x.ask else nil
	end)
	-- How it comes in (ds/orch_board_motion): from where its card stood, as
	-- the board turns into the list; from its last row, as it moves.
	local m = ctx.motion
	local placed = entry:peek()
	local built = m.build(id, placed ~= nil and Motion.IN_DRAWER[placed.place] == true)
	local start = m.start(built)
	local mounted_el: any = nil
	ui.on_cleanup(function()
		if mounted_el then
			m.detach(id, mounted_el)
		end
	end)
	local line = ui.button {
		class = "orch-list-row",
		id = `orch-row-{id}`,
		label = function(): string
			local x = e()
			return if x then x.item.title or "" else ""
		end,
		selected = function(): boolean
			return selected:get()
		end,
		w_full = true,
		min_h = 30,
		px = "$space.2",
		gap = "$space.2",
		items = "center",
		rounded = "$radius.md",
		bg = function(): string
			return if selected:get() then "$hover" else "#00000000"
		end,
		hover = { bg = "$hover" },
		active = { bg = "$pressed" },
		on_click = function()
			ctx.open(id)
		end,
		Card.Glyph {
			status = function(): string
				local x = e()
				return if x then x.status else "next"
			end,
			size = 13,
		},
		if ctx.owners
			then ui.div {
				class = "orch-list-owner",
				role = "img",
				label = function(): string
					return Card.owner_name(ctx, e())
				end,
				shrink = 0,
				hue.Dot {
					hue = function(): number?
						local x = e()
						local o = x and ctx.owner(x.item.orchestrator_id)
						return o and o.hue
					end,
					size = 7,
				},
			}
			else nil,
		ui.text {
			function(): string
				local x = e()
				return if x then x.item.title or "" else ""
			end,
			class = "orch-list-title",
			grow = 1,
			basis = 0,
			min_w = 64,
			truncate = true,
			text_size = "$font.size.sm",
			color = "$text.primary",
		},
		ui.show(function(): boolean
			return words:get() ~= ""
		end, ui.text {
			function(): string
				return words:get()
			end,
			class = "orch-list-status",
			shrink = 4,
			min_w = 0,
			max_w = 220,
			truncate = true,
			text_size = "$font.size.xs",
			color = "$text.secondary",
		}),
		ui.show(function(): boolean
			return tail:get() ~= ""
		end, ui.text {
			function(): string
				return tail:get()
			end,
			class = "orch-list-clock",
			shrink = 0,
			max_w = 160,
			truncate = true,
			tabular = true,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		}),
		Card.Threads {
			count = function(): number
				local x = e()
				return if x then #(x.item.threads or {}) else 0
			end,
		},
		if ctx.compact
			then nil
			else Card.Chip {
				chip = function(): any
					local x = e()
					return x and x.card and x.card.chip
				end,
			},
		Card.Unseen {
			shown = function(): boolean
				local x = e()
				return x ~= nil and x.card ~= nil and x.card.unseen == true and x.place == "done"
			end,
		},
	}
	-- Its question and the ask's own buttons, under its line.
	local asked = ui.show(function(): boolean
		return ask:get() ~= nil
	end, function(): Node
		return ui.column {
			class = "orch-list-ask",
			w_full = true,
			min_w = 0,
			pl = ui.raw(29, "under the title, past the glyph"),
			pr = "$space.2",
			pb = "$space.1",
			gap = "$space.1",
			ui.text {
				function(): string
					local a = ask:get()
					return if a then Card.plain(a.question or "") else ""
				end,
				class = "orch-list-question",
				w_full = true,
				min_w = 0,
				wrap = true,
				lines = 2,
				truncate = true,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$text.secondary",
			},
			Choices {
				ask = function(): any
					return ask:get()
				end,
				on_answer = function(a: any, choice: any): any
					return act.answer(a, choice)
				end,
				on_expand = if act.can("expand_ask") then act.expand else nil,
				size = "xs",
				class = "orch-list-choices",
			},
		}
	end)
	-- A paused one's Resume.
	local resume = ui.show(function(): boolean
		local x = e()
		return x ~= nil and x.status == "paused" and act.can("resume")
	end, function(): Node
		return ui.row {
			w_full = true,
			pl = ui.raw(29, "under the title, past the glyph"),
			pb = "$space.1",
			Button {
				class = "orch-list-resume",
				size = "xs",
				tone = "secondary",
				on_click = function()
					local x = entry:peek()
					if x then
						act.resume(x)
					end
				end,
				Icon { name = "action-play", size = 12 },
				locale.t("orch.board.resume"),
			},
		}
	end)
	local whole = (ContextMenu {
		label = locale.t("orch.board.card_actions"),
		ring = "$radius.md",
		w_full = true,
		shrink = 0,
		items = function(): { any }?
			local x = entry:peek()
			trace.hit("board.menu")
			return if x then act.menu(x) else nil
		end,
		ui.column { w_full = true, min_w = 0, line, asked, resume },
	}) :: Node
	return ui.div {
		w_full = true,
		min_w = 0,
		z = if built.how ~= nil then 1 else nil,
		ui.div {
			class = "orch-row-motion",
			w_full = true,
			min_w = 0,
			opacity = start,
			on_mount = function(el: any)
				mounted_el = el
				m.attach(id, el)
			end,
			on_resize = function(_size: any, el: any)
				m.placed(id, el)
			end,
			whole,
		},
	}
end)

export type SectionsProps = {
	ctx: Ctx.Ctx,
	act: Actions.Actions,
	-- The board's tab (a state): the sections it shows.
	tab: any?,
	fold: FoldState?,
}

M.Sections = ui.component(function(props: SectionsProps): Node
	local ctx, act = props.ctx, props.act
	local own_fold = ui.use_state({} :: { [string]: boolean })
	local shown = ui.use_state({} :: { [string]: number })
	local function is_open(key: string): boolean
		if props.fold then
			return props.fold.open(key, OPEN[key])
		end
		local v = own_fold:get()[key]
		if v == nil then
			return OPEN[key]
		end
		return v
	end
	local function set_open(key: string, open: boolean)
		if props.fold then
			props.fold.set(key, open)
		else
			own_fold:set_key(key, open)
		end
	end
	local function count(place: string): number
		return #ctx.ids(place)
	end
	local function cap_of(place: string): number
		return shown:get()[place] or CAP
	end
	-- Narrow: a side panel at its slimmest (by the list's own width).
	local root = ui.ref()
	local is_narrow = ui.computed(function(): boolean
		local w = root.width
		return w ~= nil and w > 0 and w < NARROW
	end)
	local function narrow(): boolean
		return is_narrow:get()
	end

	-- Rows built as the list first shows (before its first layout, when the
	-- slots first hear where they are): as many as a view's height (and
	-- MARGIN) takes, in order.
	local laid_out = false
	local reach = first_reach()
	local function guess(id: string): number
		local e = ctx.entry(id)
		if not e then
			return LINE_H
		end
		if e.place == "needs_you" and e.ask then
			return ASK_H
		elseif e.status == "paused" and act.can("resume") then
			return RESUME_H
		end
		return LINE_H
	end

	-- A row's slot: its row while near the view, else its height.
	-- (Step 3's board-to-list morph pairs a card with its row: only rows
	-- built, those in view, take part; a held slot has no box to pair.)
	local function slot(id: string): Node
		local key = `{id}:{if ctx.compact then "c" else "r"}`
		-- (A row's builder is untracked: reading its entry here follows nothing.)
		local h = HEIGHTS[key] or guess(id)
		local first = true
		if not laid_out then
			first = reach > 0
			reach -= h
		end
		local near = ui.use_state(first)
		local held = ui.use_state(h)
		return ui.div {
			class = "orch-list-slot",
			w_full = true,
			min_w = 0,
			visible_margin = MARGIN,
			on_visible = function(visible: boolean)
				laid_out = true
				if near:peek() ~= visible then
					near:set(visible)
				end
			end,
			on_resize = function(size: { w: number, h: number })
				if near:peek() and size.h > 0 then
					HEIGHTS[key] = size.h
					if math.abs(held:peek() - size.h) >= 0.5 then
						held:set(size.h)
					end
				end
			end,
			ui.switch(function(): string
				return if near:get() then "row" else "held"
			end, {
				row = function(): Node
					return Row { ctx = ctx, act = act, card = id, narrow = narrow }
				end,
				held = function(): Node
					return ui.div {
						class = "orch-list-held",
						w_full = true,
						h = function(): number
							return held:get()
						end,
					}
				end,
			}),
		}
	end

	local function rows(place: string): Node
		local ids = ui.computed(function(): { string }
			trace.hit("board.ids")
			local all = ctx.ids(place)
			local cap = cap_of(place)
			if #all <= cap then
				return all
			end
			local out: { string } = {}
			for i = 1, cap do
				table.insert(out, all[i])
			end
			return out
		end)
		local more = ui.computed(function(): number
			return math.max(0, count(place) - cap_of(place))
		end)
		return ui.column {
			w_full = true,
			min_w = 0,
			ui.each(ids, function(id: string): Node
				return slot(id)
			end, {
				key = function(id: string): string
					return id
				end,
				animate = reduce.layout({ duration = Motion.TRAVEL, ease = Motion.EASE }),
				class = "orch-list-rows",
				direction = "column",
				w_full = true,
			}),
			ui.show(function(): boolean
				return more:get() > 0
			end, function(): Node
				return ui.button {
					class = "orch-show-more",
					w_full = true,
					h = 26,
					px = "$space.2",
					items = "center",
					rounded = "$radius.md",
					bg = "#00000000",
					hover = { bg = "$hover" },
					active = { bg = "$pressed" },
					color = "$text.tertiary",
					on_click = function()
						shown:set_key(place, cap_of(place) + CAP)
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

	-- What folds under a section (Snoozed, Earlier): its own quiet head.
	local function under(place: string): Node
		return ui.show(function(): boolean
			return count(place) > 0
		end, ui.column {
			w_full = true,
			pl = "$space.3",
			Fold {
				id = `orch-list-{place}`,
				title = locale.t(`orch.board.place.{place}`),
				count = function(): number
					return count(place)
				end,
				open = function(): boolean
					return is_open(place)
				end,
				on_toggle = function(open: boolean)
					set_open(place, open)
				end,
				ui.show(function(): boolean
					return is_open(place)
				end, function(): Node
					return rows(place)
				end),
			},
		})
	end

	local function section(place: string): Node
		local sub = UNDER[place]
		return ui.column {
			class = "orch-list-section",
			w_full = true,
			min_w = 0,
			hidden = function(): boolean
				local t = if props.tab then props.tab:get() else "all"
				if t ~= "all" and not (TAB[t] and TAB[t][place]) then
					return true
				end
				-- (Planned, with nothing planned: no section.)
				return place == "planned" and count(place) == 0
			end,
			Fold {
				id = `orch-list-{place}`,
				title = locale.t(`orch.board.place.{place}`),
				count = function(): number
					return count(place)
				end,
				icon = Card.Glyph {
					status = function(): string
						return HEAD[place]
					end,
					size = 14,
				},
				open = function(): boolean
					return is_open(place)
				end,
				on_toggle = function(open: boolean)
					set_open(place, open)
				end,
				ui.column {
					w_full = true,
					min_w = 0,
					ui.show(function(): boolean
						return is_open(place)
					end, function(): Node
						return rows(place)
					end),
					ui.show(function(): boolean
						return count(place) == 0 and (sub == nil or count(sub) == 0)
					end, ui.row {
						class = "orch-list-empty",
						w_full = true,
						h = 30,
						px = "$space.2",
						items = "center",
						ui.text {
							locale.t(`orch.board.none.{place}`),
							text_size = "$font.size.sm",
							color = "$text.tertiary",
						},
					}),
					if sub then under(sub) else nil,
				},
			},
		}
	end

	return ui.column {
		ref = root,
		id = "orch-list",
		class = "orch-list",
		w_full = true,
		min_w = 0,
		gap = "$space.2",
		section("needs_you"),
		section("running"),
		section("planned"),
		section("done"),
	}
end)

export type Props = Ctx.Props

-- The list by itself: the whole of it, from a host's rows.
M.List = ui.component(function(props: Props): Node
	local ctx = Ctx.make(props)
	local act = Actions.make(ctx)
	return ui.column {
		id = props.id or "orch-board-list",
		class = "orch-board-list",
		w_full = true,
		min_w = 0,
		M.Sections { ctx = ctx, act = act, tab = props.tab, fold = props.fold },
		ctx.motion.host(),
	}
end)

return M

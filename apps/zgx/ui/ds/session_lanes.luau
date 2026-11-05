--!strict
-- A session's lanes, as the shipping app's details card lists them
-- (crates/ui shell/session_lanes.rs): its side chats and its subagents.
--
-- Side chats, newest first. With none yet, "New side chat" and "Fork chat"
-- are its two rows; once there are some, they're icon buttons on its
-- heading. Subagents (only when there are any), under a rule: the running
-- ones first, then those not yet heard from, and the finished ones folded
-- into one row, "{n} finished", that opens them. Each list shows 8 rows,
-- then "Show N more" (10 at a time). A row: its state (a spinner working, a
-- check done, a dot otherwise), its agent's mark, its title and how long
-- ago. What's folded and how far a list is shown start again for another
-- session.
--
--   SessionLanes {
--     session = function() return id end,      -- whose lanes (resets the folds)
--     side = function() return { Child } end,  -- { id, title, status?, agent?, updated }
--     subagents = function() return { Sub } end, -- { ref, title, status?, agent?, at? }
--     on_new = fn, on_fork = fn, on_open_side = fn(child), on_open_subagent = fn(sub),
--   }

local Icon = require("@self/icon")
local IconButton = require("@self/icon_button").IconButton
local Mark = require("@self/mark")
local control = require("@self/control")
local status = require("@self/status")
local locale = require("@self/locale")

export type Child = { id: string, title: string, status: string?, agent: string?, updated: number? }
export type Sub = { ref: string, title: string, status: string?, agent: string?, at: number? }
export type Props = {
	session: () -> string?,
	side: () -> { Child },
	subagents: () -> { Sub },
	on_new: () -> (),
	on_fork: () -> (),
	on_open_side: (child: Child) -> (),
	on_open_subagent: (sub: Sub) -> (),
	w: number?,
}

-- Rows a list shows at first, and how many more each "Show more" adds.
local FIRST, PAGE = 8, 10
-- The card's corner and its inset: its rows' corner nests inside both.
local CARD_RADIUS = "$radius.xl"
local INSET = "$space.1"

local function row_radius(): any
	return ui.raw(control.nest(CARD_RADIUS, 1, INSET), "a row's corner, nested in the card's")
end

-- A lane's state, as a mark: working spins, done is a check, the rest a dot.
local function mark(kind: () -> string?): Node
	return ui.switch(function(): string
		local k = kind()
		return if k == "working" or k == "running" then "working" elseif k == "done" then "done" else "dot"
	end, {
		working = status.StatusIcon { kind = "working", size = 14 },
		done = status.StatusIcon { kind = "done", size = 14 },
		dot = ui.row { w = 14, h = 14, items = "center", justify = "center", ui.div { w = 5, h = 5, rounded = "$radius.full", bg = "$text.tertiary" } },
	})
end

-- One lane's row: what it is, whose, its title and when.
local function lane(props: {
	label: () -> string,
	name: () -> string,
	kind: () -> string?,
	agent: () -> string?,
	at: () -> number?,
	fallback_icon: Icon.Name,
	on_click: () -> (),
}): Node
	local hovered = ui.use_state(false)
	return ui.button {
		label = props.label,
		w_full = true,
		h = 30,
		px = "$space.2",
		gap = "$space.2",
		items = "center",
		rounded = row_radius(),
		bg = function(): string
			return if hovered:get() then "$hover" else "#00000000"
		end,
		hover = {},
		active = {},
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		on_click = props.on_click,
		mark(props.kind),
		ui.switch(function(): string
			return if props.agent() then "mark" else "icon"
		end, {
			mark = Mark {
				name = function(): any
					return props.agent() or "claude"
				end,
				size = 14,
				bare = true,
			},
			icon = ui.div { color = "$text.tertiary", Icon { name = props.fallback_icon, size = 14 } },
		}),
		ui.text { props.name, grow = 1, min_w = 0, truncate = true, text_size = "$font.size.sm", color = "$text.primary" },
		ui.text {
			function(): string
				local at = props.at()
				return if at then status.ago(os.time() - at) else ""
			end,
			shrink = 0,
			tabular = true,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		},
	}
end

-- An action row (a lane list with nothing in it yet).
local function action(icon: Icon.Name, words: Message, on_click: () -> ()): Node
	local hovered = ui.use_state(false)
	return ui.button {
		label = words,
		w_full = true,
		h = 30,
		px = "$space.2",
		gap = "$space.2",
		items = "center",
		rounded = row_radius(),
		color = "$text.secondary",
		bg = function(): string
			return if hovered:get() then "$hover" else "#00000000"
		end,
		hover = {},
		active = {},
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		on_click = on_click,
		Icon { name = icon, size = 14 },
		ui.text { words, text_size = "$font.size.sm" },
	}
end

local function heading(words: Message, trail: Node?): Node
	return ui.row {
		w_full = true,
		h = 28,
		px = "$space.2",
		items = "center",
		ui.text { words, grow = 1, text_size = "$font.size.xs", weight = "$font.weight.medium", color = "$text.tertiary" },
		trail,
	}
end

return ui.component(function(props: Props): Node
	local side_shown = ui.use_state(FIRST)
	local subs_shown = ui.use_state(FIRST)
	local finished_open = ui.use_state(false)
	-- Another session: its lists from the top, its finished folded.
	ui.effect(function()
		props.session()
		side_shown:set(FIRST)
		subs_shown:set(FIRST)
		finished_open:set(false)
	end)

	local side = ui.computed(function(): { Child }
		local all = props.side()
		local out = {}
		for i = 1, math.min(#all, side_shown:get()) do
			table.insert(out, all[i])
		end
		return out
	end)
	-- Subagents: running first (oldest spawn first), then not yet heard
	-- from; the finished (done or failed), newest first, folded apart.
	local split = ui.computed(function(): { active: { Sub }, finished: { Sub } }
		local running, quiet, finished = {}, {}, {}
		for _, s in props.subagents() do
			if s.status == "running" then
				table.insert(running, s)
			elseif s.status == "done" or s.status == "failed" then
				table.insert(finished, 1, s)
			else
				table.insert(quiet, s)
			end
		end
		for _, s in quiet do
			table.insert(running, s)
		end
		return { active = running, finished = finished }
	end)
	local active = ui.computed(function(): { Sub }
		local all = split:get().active
		local out = {}
		for i = 1, math.min(#all, subs_shown:get()) do
			table.insert(out, all[i])
		end
		return out
	end)
	local finished = ui.computed(function(): { Sub }
		return if finished_open:get() then split:get().finished else {}
	end)

	local function side_row(_c: Child, _i: number, now: State<any>): Node
		return lane {
			label = function(): string
				return locale.s("ds.session_lanes.open_side", { title = now:get().title })
			end,
			name = function(): string
				return now:get().title
			end,
			kind = function(): string?
				return now:get().status
			end,
			agent = function(): string?
				return now:get().agent
			end,
			at = function(): number?
				return now:get().updated
			end,
			fallback_icon = "side-chat",
			on_click = function()
				props.on_open_side(now:peek())
			end,
		}
	end
	local function sub_row(_s: Sub, _i: number, now: State<any>): Node
		return lane {
			label = function(): string
				return locale.s("ds.session_lanes.open_subagent", { title = now:get().title })
			end,
			name = function(): string
				return now:get().title
			end,
			kind = function(): string?
				return now:get().status
			end,
			agent = function(): string?
				return now:get().agent
			end,
			at = function(): number?
				return now:get().at
			end,
			fallback_icon = "bot",
			on_click = function()
				props.on_open_subagent(now:peek())
			end,
		}
	end
	local function by_id(c: Child): string
		return c.id
	end
	local function by_ref(s: Sub): string
		return s.ref
	end

	local function show_more(shown: State<number>, total: () -> number): Node
		return ui.show(function(): boolean
			return total() > shown:get()
		end, ui.row {
			w_full = true,
			ui.button {
				label = function(): string
					return locale.s("ds.session_lanes.show_more", { n = math.min(total() - shown:get(), PAGE) })
				end,
				w_full = true,
				h = 28,
				px = "$space.2",
				items = "center",
				rounded = row_radius(),
				bg = "#00000000",
				hover = { bg = "$hover" },
				on_click = function()
					shown:set(shown:peek() + PAGE)
				end,
				ui.text {
					function(): string
						return locale.s("ds.session_lanes.show_more", { n = math.min(total() - shown:get(), PAGE) })
					end,
					text_size = "$font.size.sm",
					color = "$text.secondary",
				},
			},
		})
	end

	return ui.column {
		id = "session-lanes",
		w = props.w or 280,
		p = INSET,
		gap = "$zeron.list.gap",
		rounded = CARD_RADIUS,
		border = 1,
		border_color = "$zeron.settings.edge",
		bg = "$zeron.settings.card",
		heading(
			locale.t("ds.session_lanes.side_chats"),
			ui.show(function(): boolean
				return #props.side() > 0
			end, ui.row {
				gap = "$zeron.space.hair",
				IconButton { icon = "plus", label = locale.t("ds.session_lanes.new_side"), tooltip = locale.s("ds.session_lanes.new_side"), size = "sm", tone = "quiet", on_click = props.on_new },
				IconButton { icon = "git-branch", label = locale.t("ds.session_lanes.fork"), tooltip = locale.s("ds.session_lanes.fork"), size = "sm", tone = "quiet", on_click = props.on_fork },
			})
		),
		ui.show(function(): boolean
			return #props.side() == 0
		end, ui.column {
			w_full = true,
			gap = "$zeron.list.gap",
			action("plus", locale.t("ds.session_lanes.new_side"), props.on_new),
			action("git-branch", locale.t("ds.session_lanes.fork"), props.on_fork),
		}),
		ui.column { w_full = true, gap = "$zeron.list.gap", ui.each(side, side_row, { key = by_id, w_full = true }) },
		show_more(side_shown, function(): number
			return #props.side()
		end),
		ui.show(function(): boolean
			local s = split:get()
			return #s.active + #s.finished > 0
		end, ui.column {
			w_full = true,
			gap = "$zeron.list.gap",
			ui.div { w_full = true, h = 1, my = "$space.1", bg = "$zeron.settings.rule" },
			heading(locale.t("ds.session_lanes.subagents")),
			ui.column { w_full = true, gap = "$zeron.list.gap", ui.each(active, sub_row, { key = by_ref, w_full = true }) },
			show_more(subs_shown, function(): number
				return #split:get().active
			end),
			ui.show(function(): boolean
				return #split:get().finished > 0
			end, ui.button {
				label = function(): string
					local n = #split:get().finished
					return locale.s(if finished_open:get() then "ds.session_lanes.collapse_finished" else "ds.session_lanes.expand_finished", { n = n })
				end,
				expanded = function(): boolean
					return finished_open:get()
				end,
				w_full = true,
				h = 28,
				px = "$space.2",
				gap = "$space.2",
				items = "center",
				rounded = row_radius(),
				bg = "#00000000",
				hover = { bg = "$hover" },
				on_click = function()
					finished_open:set(not finished_open:peek())
				end,
				ui.div {
					color = "$text.tertiary",
					rotate = ui.spring(finished_open, { from = 0, to = 90, stiffness = 380, damping = 30 }),
					Icon { name = "alt-arrow-right", size = 14 },
				},
				ui.text {
					function(): string
						return locale.s("ds.session_lanes.finished", { n = #split:get().finished })
					end,
					text_size = "$font.size.sm",
					color = "$text.secondary",
				},
			}),
			ui.column { w_full = true, gap = "$zeron.list.gap", ui.each(finished, sub_row, { key = by_ref, w_full = true }) },
		}),
	}
end)

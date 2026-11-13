-- The Control room (`/control`): the Brain dump's home, and the one place
-- that shows everything orchestration is doing. Its widgets, in order:
-- Capture (dump anything; the newest dump's receipt under it, older ones
-- folded), Needs you, the Map, the Board, Machines and Comms. Two columns
-- from 1100 points (Capture, the Map and the Board on the left; Needs you,
-- Machines and Comms on the right), one below that. Each widget can be
-- hidden and moved; that's this device's (`control_widgets`). Plugins add
-- widgets of their own at `slot:zeron.orchestration.widgets`.
--
--   Page()    the page, as the shell mounts it

local ds = require("@ds")
local Icon = require("@ds/icon")
local IconButton = require("@ds/icon_button").IconButton
local ContextMenu = require("@ds/context_menu")
local locale = require("@ds/locale")
local status = require("@ds/status")
local hue = require("@ds/orch_hue")
local data = require("@ds/orch_data")
local Receipt = require("@ds/orch_receipt")
local Asks = require("@ds/orch_asks")
local OrchMap = require("@ds/orch_map")
local Machines = require("@ds/orch_machines")
local Comms = require("@ds/orch_comms")
local orchestration = require("@account/orchestration")
local sessions = require("@account/sessions")
local route = require("@app/screens/shell/route")

local M = {}

-- Where the page goes to two columns.
M.WIDE = 1100
-- The right column's width, wide.
local SIDE_W = 400

-- The widgets: each one's column when wide.
M.WIDGETS = { "capture", "needs", "map", "board", "machines", "comms" }
local COLUMN: { [string]: string } = { capture = "left", map = "left", board = "left", needs = "right", machines = "right", comms = "right" }

-- Shown and in what order, on this device.
export type Widgets = { order: { string }, hidden: { [string]: boolean } }
M.widgets = ui.persist("control_widgets", { order = table.clone(M.WIDGETS), hidden = {} } :: Widgets)

local function full_order(w: Widgets): { string }
	local out, seen = {}, {}
	for _, id in w.order or {} do
		if COLUMN[id] and not seen[id] then
			seen[id] = true
			table.insert(out, id)
		end
	end
	for _, id in M.WIDGETS do
		if not seen[id] then
			table.insert(out, id)
		end
	end
	return out
end

function M.toggle(id: string)
	local w = M.widgets:peek()
	local hidden = table.clone(w.hidden or {})
	hidden[id] = if hidden[id] then nil else true
	M.widgets:set({ order = full_order(w), hidden = hidden })
end

-- A widget one place up (-1) or down (1), past those hidden, and, with the
-- page wide, past those in the other column.
function M.move(id: string, by: number, wide: boolean)
	local w = M.widgets:peek()
	local list = full_order(w)
	local at = table.find(list, id)
	if not at then
		return
	end
	local hidden = w.hidden or {}
	local j = at + by
	while list[j] and (hidden[list[j]] or (wide and COLUMN[list[j]] ~= COLUMN[id])) do
		j += by
	end
	if not list[j] then
		return
	end
	list[at], list[j] = list[j], list[at]
	M.widgets:set({ order = list, hidden = table.clone(hidden) })
end

-- The draft, kept as the page comes and goes.
local draft = ui.state("control_draft", "")
-- The dump shown in Capture's receipt (nil: the newest).
local picked = ui.state("control_dump_picked", nil :: string?)
local history_open = ui.state("control_history_open", false)

local chats_now = ui.computed(function(): { [string]: any }
	return data.chats(sessions.now:get())
end)

local function world(): any
	return {
		now = orchestration.now:get(),
		chats = chats_now:get(),
		devices = sessions.devices:get(),
		loads = orchestration.loads:get(),
		here = sessions.here:get(),
	}
end

local function name_of(id: string?): string?
	return data.name_of(orchestration.now:get(), chats_now:get(), id)
end
local function hue_of(id: string?): number?
	return data.hue_of(orchestration.now:get(), chats_now:get(), id)
end

-- A chat opened: an orchestrator's on its own page, any other as a session.
function M.open_chat(id: string)
	for _, o in orchestration.now:peek().orchestrators or {} do
		if o.id == id then
			route.orchestrator(id)
			return
		end
	end
	route.open_session(id)
end

local function open_target(t: { kind: string, id: string? })
	if not t.id or t.id == "" then
		return
	end
	if t.kind == "orchestrator" or t.kind == "new_orchestrator" then
		route.orchestrator(t.id :: string)
	elseif t.kind == "chat" or t.kind == "new_thread" then
		M.open_chat(t.id :: string)
	end
end

local function said(what: string): (any) -> ()
	return function(_err: any)
		ds.toast.show { title = locale.s(what), tone = "danger" }
	end
end

function M.send()
	local text = string.match(draft:peek(), "^%s*(.-)%s*$") or ""
	if text == "" then
		return
	end
	draft:set("")
	picked:set(nil)
	orchestration.dump(text):after(function() end, function(e: any)
		-- (Not taken: the words back, to send again.)
		if draft:peek() == "" then
			draft:set(text)
		end
		said("control.capture.failed")(e)
	end)
end

-- Capture: the field, and the receipt of the dump shown.
local function capture(): Node
	local shown = ui.computed(function(): any
		local dumps = orchestration.dumps:get()
		local id = picked:get()
		if id then
			for _, d in dumps do
				if d.id == id then
					return d
				end
			end
		end
		return dumps[1]
	end)
	local older = ui.computed(function(): { any }
		local s = shown:get()
		local out = {}
		for _, d in orchestration.dumps:get() do
			if not s or d.id ~= s.id then
				table.insert(out, d)
			end
		end
		return out
	end)
	local function empty(): boolean
		return string.match(draft:get(), "%S") == nil
	end
	return ui.column {
		id = "control-capture",
		w_full = true,
		gap = "$space.5",
		ui.column {
			w_full = true,
			relative = true,
			ds.TextArea {
				value = draft,
				name = locale.t("control.capture.name"),
				placeholder = locale.t("control.capture.placeholder"),
				rows = 3,
				max_rows = 12,
				size = "lg",
				w_full = true,
				on_key = function(e: any): boolean?
					if e.key == "Enter" and not e.shift and not e.alt then
						M.send()
						return true
					end
					return nil
				end,
			},
			ui.row {
				absolute = true,
				right = 8,
				bottom = 8,
				gap = "$space.1",
				items = "center",
				ds.Button {
					id = "control-capture-send",
					tone = "primary",
					size = "sm",
					shape = "pill",
					icon = true,
					label = locale.t("control.capture.send"),
					tooltip = locale.s("control.capture.send"),
					disabled = empty,
					on_click = M.send,
					Icon { name = "arrow-up", size = 16 },
				},
			},
		},
		ui.show(function(): boolean
			return shown:get() ~= nil
		end, ui.column {
			w_full = true,
			gap = "$space.3",
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				ui.text {
					function(): string
						local d = shown:get()
						return if d then status.ago(status.since(d.created or 0)) else ""
					end,
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
				ui.show(function(): boolean
					return picked:get() ~= nil
				end, ds.Button {
					tone = "ghost",
					size = "xs",
					on_click = function()
						picked:set(nil)
					end,
					locale.t("control.capture.newest"),
				}),
			},
			Receipt {
				dump = function(): any
					return shown:get()
				end,
				now = function(): any
					return orchestration.now:get()
				end,
				chats = function(): { [string]: any }
					return chats_now:get()
				end,
				on_reroute = function(dump_id: string, route_id: string, target: any)
					orchestration.reroute(dump_id, route_id, target):after(function() end, said("control.capture.move_failed"))
				end,
				on_open = open_target,
				text_size = "$font.size.lg",
				line_height = "$font.line.lg",
			},
		}),
		-- Earlier dumps, folded.
		ui.show(function(): boolean
			return #older:get() > 0
		end, ui.column {
			w_full = true,
			gap = "$space.1",
			ui.button {
				id = "control-history-toggle",
				label = function(): string
					return locale.s("control.capture.earlier", { n = #older:get() })
				end,
				expanded = function(): boolean
					return history_open:get()
				end,
				h = 26,
				px = 0,
				gap = "$space.1",
				items = "center",
				bg = "#00000000",
				on_click = function()
					history_open:set(not history_open:peek())
				end,
				ui.div {
					color = "$text.tertiary",
					Icon {
						name = function(): any
							return if history_open:get() then "alt-arrow-down" else "alt-arrow-right"
						end,
						size = 12,
					},
				},
				ui.text {
					function(): string
						return locale.s("control.capture.earlier", { n = #older:get() })
					end,
					text_size = "$font.size.sm",
					color = "$text.secondary",
				},
			},
			ui.show(function(): boolean
				return history_open:get()
			end, ui.each(older, function(_d: any, _i: number, row: State<any>): Node
				return ui.button {
					class = "control-history-row",
					label = function(): string
						return row:get().text or ""
					end,
					w_full = true,
					h = 30,
					px = "$space.2",
					gap = "$space.3",
					items = "center",
					rounded = "$radius.md",
					bg = "#00000000",
					hover = { bg = "$hover" },
					on_click = function()
						picked:set(row:peek().id)
					end,
					ui.text {
						function(): string
							return row:get().text or ""
						end,
						literal = true,
						grow = 1,
						min_w = 0,
						truncate = true,
						text_size = "$font.size.sm",
						color = "$text.secondary",
					},
					-- Where its parts went.
					ui.each(ui.computed(function(): { any }
						return row:get().routes or {}
					end), function(_r: any, _j: number, rr: State<any>): Node
						return hue.Dot { hue = function(): number?
							local t = data.target(orchestration.now:get(), chats_now:get(), rr:get().target, function(k: string): string
								return k
							end)
							return t.hue
						end, size = 6 }
					end, { key = function(r: any): string
						return r.id
					end, direction = "row", gap = "$space.1", items = "center", shrink = 0 }),
					ui.show(function(): boolean
						return row:get().state == "needs_you"
					end, ui.div { color = "$zeron.status.input", shrink = 0, Icon { name = "status-input", size = 12 } }),
					ui.text {
						function(): string
							return status.ago(status.since(row:get().created or 0))
						end,
						tabular = true,
						shrink = 0,
						text_size = "$font.size.xs",
						color = "$text.tertiary",
					},
				}
			end, { key = function(d: any): string
				return d.id
			end, w_full = true })),
		}),
	}
end

-- The board: a stand-in for ds/orch_board (the orchestrator stream's), its
-- columns and their cards' titles.
-- TODO(integration): mount ds/orch_board here once orch/ui-orchestrator is in.
local BOARD = { "active", "planned", "blocked", "review", "done" }
local RANK: { [string]: number } = { urgent = 0, high = 1, normal = 2, low = 3 }
local function board(): Node
	local function column(state: string): Node
		local cards = ui.computed(function(): { any }
			local out = {}
			local day = os.time() - 86400
			for _, it in orchestration.now:get().items or {} do
				if it.state == state and (state ~= "done" or (it.done or it.updated or 0) >= day) then
					table.insert(out, it)
				end
			end
			table.sort(out, function(a: any, b: any): boolean
				local pa, pb = RANK[a.priority] or 2, RANK[b.priority] or 2
				if pa ~= pb then
					return pa < pb
				end
				return (a.order or 0) < (b.order or 0)
			end)
			return out
		end)
		return ui.column {
			class = "control-board-column",
			grow = 1,
			basis = 0,
			min_w = 0,
			gap = "$space.2",
			ui.row {
				gap = "$space.2",
				items = "center",
				ui.text { locale.t(`control.board.{state}`), text_size = "$font.size.xs", weight = "$font.weight.medium", color = "$text.tertiary" },
				ui.text {
					function(): string
						local n = #cards:get()
						return if n > 0 then tostring(n) else ""
					end,
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			},
			ui.each(cards, function(_it: any, _i: number, row: State<any>): Node
				return ui.row {
					class = "control-board-card",
					w_full = true,
					gap = "$space.2",
					py = "$zeron.space.hair",
					items = "start",
					ui.div {
						w = 2,
						h = 16,
						mt = "$zeron.space.hair",
						shrink = 0,
						rounded = "$radius.full",
						bg = function(): any
							return hue.paint(hue_of(row:get().orchestrator_id), 0.9)
						end,
					},
					ui.text {
						function(): string
							return row:get().title or ""
						end,
						grow = 1,
						min_w = 0,
						wrap = true,
						text_size = "$font.size.sm",
						color = if state == "done" then "$text.secondary" else "$text.primary",
					},
					ui.show(function(): boolean
						local p = row:get().priority
						return p == "urgent" or p == "high"
					end, ui.div {
						color = function(): string
							return if row:get().priority == "urgent" then "$danger" else "$text.tertiary"
						end,
						shrink = 0,
						Icon { name = "flag", size = 12 },
					}),
				}
			end, { key = function(it: any): string
				return it.id
			end, w_full = true, gap = "$space.1" }),
		}
	end
	local cols = {}
	for _, s in BOARD do
		table.insert(cols, column(s))
	end
	local p: any = { id = "control-board", w_full = true, gap = "$space.5", items = "start" }
	for _, c in cols do
		table.insert(p, c)
	end
	return ui.row(p)
end

local function needs(): Node
	return Asks {
		needs = function(): { any }
			return data.needs(orchestration.now:get(), sessions.now:get())
		end,
		name_of = name_of,
		on_answer = function(ask: any, text: string)
			orchestration.answer(ask.id, text):after(function() end, said("control.needs.failed"))
		end,
		on_dismiss = function(ask: any)
			orchestration.dismiss(ask.id):after(function() end, said("control.needs.failed"))
		end,
		on_open = M.open_chat,
	}
end

local function map(): Node
	return OrchMap.Map {
		world = world,
		comms = function(): { any }
			return orchestration.comms:get()
		end,
		on_open_chat = M.open_chat,
		on_open_orchestrator = function(id: string)
			route.orchestrator(id)
		end,
	}
end

local function machines(): Node
	return Machines {
		devices = function(): { any }
			return sessions.devices:get()
		end,
		loads = function(): { [string]: any }
			return orchestration.loads:get()
		end,
		chats = function(): { [string]: any }
			return chats_now:get()
		end,
		here = function(): string?
			return sessions.here:get()
		end,
		hue_of = function(id: string): number?
			return hue_of(id)
		end,
		on_open_chat = M.open_chat,
	}
end

local function comms(): Node
	return Comms {
		comms = function(): { any }
			return orchestration.comms:get()
		end,
		name_of = function(id: string): string?
			return name_of(id)
		end,
		hue_of = function(id: string): number?
			return hue_of(id)
		end,
		on_open = M.open_chat,
	}
end

local BUILD: { [string]: () -> Node } = { capture = capture, needs = needs, map = map, board = board, machines = machines, comms = comms }

-- What a widget's title has beside it: how many wait, how many work.
local function count_of(id: string): string
	if id == "needs" then
		local n = #data.needs(orchestration.now:get(), sessions.now:get())
		return if n > 0 then tostring(n) else ""
	elseif id == "map" then
		local working = 0
		for _, m in orchestration.now:get().crew or {} do
			local c = chats_now:get()[m.chat_id]
			if not m.released and c and c.status == "working" then
				working += 1
			end
		end
		return if working > 0 then locale.s("control.map.working", { n = working }) else ""
	end
	return ""
end

-- A menu opened under an element of ours (a ⋯), as from a click there.
local function opens_under(): (((x: number, y: number) -> boolean) -> (), (_v: boolean, el: Element) -> (), () -> ())
	local opener: ((x: number, y: number) -> boolean)? = nil
	local at: Element? = nil
	return function(open_at)
		opener = open_at
	end, function(_v, el)
		at = el
	end, function()
		local el = at
		if opener and el and el.x and el.y then
			(opener :: any)(el.x, el.y + (el.height or 22) + 4)
		end
	end
end

-- One widget: its title (a count beside it, its menu at its end) over it.
local function widget(id: string, wide: boolean): Node
	local hook, seen, open = opens_under()
	local function entries(): { any }
		return {
			{ label = locale.s("control.widgets.up"), icon = "arrow-up", on_select = function()
				M.move(id, -1, wide)
			end },
			{ label = locale.s("control.widgets.down"), icon = "arrow-down", on_select = function()
				M.move(id, 1, wide)
			end },
			{ separator = true },
			{ label = locale.s("control.widgets.hide"), icon = "eye-closed", on_select = function()
				M.toggle(id)
			end },
		}
	end
	return ui.column {
		id = `control-widget-{id}`,
		class = "control-widget",
		w_full = true,
		gap = "$space.3",
		ContextMenu {
			items = entries,
			hook = hook,
			label = locale.t(`control.widget.{id}`),
			w_full = true,
			ui.row {
				w_full = true,
				h = 24,
				gap = "$space.2",
				items = "center",
				ds.SectionTitle { locale.t(`control.widget.{id}`) },
				ui.text {
					function(): string
						return count_of(id)
					end,
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
				ui.div { grow = 1 },
				ui.div {
					class = "control-widget-menu",
					on_visible = seen,
					IconButton {
						icon = "more-horizontal",
						label = locale.t("control.widgets.menu"),
						size = "sm",
						tone = "quiet",
						on_click = open,
					},
				},
			},
		},
		BUILD[id](),
	}
end

-- The widgets shown, in their order, of one column (nil: all of them).
local function shown_of(column: string?): any
	return ui.computed(function(): { { id: string } }
		local w = M.widgets:get()
		local hidden = w.hidden or {}
		local out = {}
		for _, id in full_order(w) do
			if not hidden[id] and (column == nil or COLUMN[id] == column) then
				table.insert(out, { id = id })
			end
		end
		return out
	end)
end

local function by_id(w: { id: string }): string
	return w.id
end

local function slot(): Node
	return ui.column {
		id = "control-plugin-widgets",
		w_full = true,
		ui.slot("zeron.orchestration.widgets", { direction = "column", gap = "$space.7", w_full = true }),
	}
end

local function layout(kind: string): Node
	local wide = kind == "wide"
	local function list(column: string?): Node
		return ui.each(shown_of(column), function(w: { id: string }): Node
			return widget(w.id, wide)
		end, { key = by_id, w_full = true, gap = "$space.7" })
	end
	if wide then
		return ui.row {
			id = "control-columns",
			class = "control-wide",
			w_full = true,
			gap = "$space.7",
			items = "start",
			ui.column { id = "control-left", grow = 1, basis = 0, min_w = 0, gap = "$space.7", list("left") },
			ui.column { id = "control-right", w = SIDE_W, shrink = 0, gap = "$space.7", list("right"), slot() },
		}
	end
	return ui.column {
		id = "control-columns",
		class = "control-narrow",
		w_full = true,
		gap = "$space.7",
		list(nil),
		slot(),
	}
end

-- The page's head: its name, what's going on, and the widgets' menu.
local function head(): Node
	local hook, seen, open = opens_under()
	local function entries(): { any }
		local out: { any } = { { heading = locale.s("control.widgets.title") } }
		for _, id in M.WIDGETS do
			table.insert(out, {
				label = locale.s(`control.widget.{id}`),
				keep_open = true,
				checked = function(): boolean
					return not (M.widgets:get().hidden or {})[id]
				end,
				on_select = function()
					M.toggle(id)
				end,
			})
		end
		table.insert(out, { separator = true })
		table.insert(out, { label = locale.s("control.widgets.reset"), icon = "refresh", on_select = function()
			M.widgets:set({ order = table.clone(M.WIDGETS), hidden = {} })
		end })
		return out
	end
	local function summary(): string
		local now = orchestration.now:get()
		local n, working = 0, 0
		for _, o in now.orchestrators or {} do
			if o.kind ~= "brain_dump" and not o.archived then
				n += 1
			end
		end
		for _, m in now.crew or {} do
			local c = chats_now:get()[m.chat_id]
			if not m.released and c and c.status == "working" then
				working += 1
			end
		end
		local needing = #data.needs(now, sessions.now:get())
		return locale.s("control.summary", { orchestrators = n, working = working, needs = needing })
	end
	return ContextMenu {
		items = entries,
		hook = hook,
		label = locale.t("control.widgets.title"),
		w_full = true,
		ui.row {
			id = "control-head",
			w_full = true,
			gap = "$space.3",
			items = "center",
			ds.Text { locale.t("control.title"), style = "heading" },
			ui.text { summary, tabular = true, text_size = "$font.size.sm", color = "$text.tertiary", truncate = true, min_w = 0 },
			ui.div { grow = 1 },
			ui.div {
				on_visible = seen,
				IconButton {
					id = "control-widgets-menu",
					icon = "tuning",
					label = locale.t("control.widgets.title"),
					size = "md",
					tone = "quiet",
					on_click = open,
				},
			},
		},
	}
end

M.Page = ui.component(function(): Node
	local width = ui.use_state(0)
	-- Loads are sampled only while the page shows (Machines, the Map's lanes).
	orchestration.loads_wanted(true)
	ui.on_cleanup(function()
		orchestration.loads_wanted(false)
	end)
	return ui.column {
		id = "control-room",
		w_full = true,
		h_full = true,
		min_h = 0,
		ui.scroll {
			id = "control-scroll",
			w_full = true,
			grow = 1,
			min_h = 0,
			fade = { 12, 12 },
			fade_overflow = true,
			ui.column {
				w_full = true,
				items = "center",
				px = "$space.6",
				pt = "$space.4",
				pb = "$space.8",
				on_resize = function(size: { w: number, h: number })
					if math.abs(size.w - width:peek()) >= 1 then
						width:set(size.w)
					end
				end,
				ui.column {
					w_full = true,
					max_w = 1480,
					gap = "$space.6",
					head(),
					ui.presence(function(): string
						-- (A real swap: two columns or one, the widgets laid out again.)
						return if width:get() >= M.WIDE then "wide" else "narrow"
					end, layout, { w_full = true }),
				},
			},
		},
	}
end)

return M

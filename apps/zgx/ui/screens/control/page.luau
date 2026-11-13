-- The Control room (`/control`): the Brain dump's home, and the one place
-- that shows everything orchestration is doing. Its widgets, in order:
-- Capture (dump anything; the newest dump's receipt under it, older ones
-- folded), Needs you, Q&A (the person's questions and their answers), the
-- Map, the Board, Machines and Comms. Two columns
-- from 1100 points (Capture and the Map on the left; Needs you, Machines and
-- Comms on the right; the Board across both, under them), one below that. Each widget can be
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
local MapMenu = require("@app/screens/control/map_menu")
local Machines = require("@ds/orch_machines")
local Name = require("@ds/orch_name")
local Comms = require("@ds/orch_comms")
local OrchBoard = require("@ds/orch_board")
local QA = require("@ds/orch_qa")
local Links = require("@ds/links")
local Block = require("@ds/orch_block")
local HoverCard = require("@ds/hover_card")
local Counter = require("@ds/orch_counter")
local Progress = require("@app/screens/control/progress")
local orchestration = require("@account/orchestration")
local sessions = require("@account/sessions")
local route = require("@app/screens/shell/route")
local LaneTab = require("@app/screens/chat/lane_tab")
local KeptScroll = require("@ds/kept_scroll")

local M = {}

-- Where the page goes to two columns.
M.WIDE = 1100

-- Where the page is scrolled: kept as the page is made again (a thread
-- opened from it and back) and as its widgets change under it.
M.place = ui.state("control_scroll", 0)
-- The right column's width, wide.
local SIDE_W = 420
-- Between blocks: enough that each reads as its own.
local GAP = "$space.4"

-- The widgets: each one's column when wide.
M.WIDGETS = { "capture", "needs", "qa", "progress", "map", "board", "machines", "comms" }
-- (The Board spans both, under them: its columns want the room.)
local COLUMN: { [string]: string } = { capture = "left", map = "left", board = "full", needs = "right", qa = "right", progress = "right", machines = "right", comms = "right" }

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
	-- (One this device hasn't placed yet, new since: after the one before
	-- it in the default order.)
	for i, id in M.WIDGETS do
		if not seen[id] then
			seen[id] = true
			local after = if i > 1 then table.find(out, M.WIDGETS[i - 1]) else 0
			table.insert(out, (after or #out) + 1, id)
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

-- Where a dump's receipt stands, for a board card's source to show it: the
-- newest's, and each earlier one's row, by id.
local newest_ref = ui.ref()
local history_refs: { [string]: any } = {}

local history_open = ui.state("control_history_open", false)
-- The earlier dumps opened in the history, by id.
local history_opened = ui.state("control_history_opened", {} :: { [string]: boolean })

local chats_now = ui.computed(function(): { [string]: any }
	return data.chats(sessions.now:get())
end)
-- The open asks, by the chat that asked: a card whose thread asked waits on
-- the person.
local asks_now = ui.computed(function(): { [string]: any }
	return data.open_asks(orchestration.now:get())
end)

local function world(): any
	return {
		now = orchestration.now:get(),
		chats = chats_now:get(),
		devices = sessions.devices:get(),
		projects = sessions.projects:get(),
		loads = orchestration.loads:get(),
		here = sessions.here:get(),
	}
end

local function name_of(id: string?): string?
	return data.name_of(orchestration.now:get(), chats_now:get(), id)
end
-- A thread's name as the Map and the sidebar give it (ds/orch_name).
local named = ui.computed(function(): any
	return Name.index(orchestration.now:get())
end)
local function thread_name(id: string): string
	return Name.from(named:get(), chats_now:get(), id)
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

-- The Brain dump's chat: Capture's composer is its own (its model and
-- agent the Brain dump's), every send a dump.
local function brain_dump(): string?
	for _, o in orchestration.now:get().orchestrators or {} do
		if o.kind == "brain_dump" then
			return o.id
		end
	end
	return nil
end

-- A send from Capture: the words, the files they carry, and the Brain
-- dump's model as the composer has it picked, a dump.
function M.take(text: string, attachments: { any }, picks: { [string]: any }?): any
	return orchestration.dump(text, attachments, brain_dump(), picks)
end

-- A dump's receipt (ds/orch_receipt): the newest one's, or an earlier
-- one's opened in the history (`settled`: its parts there at once).
local function receipt(dump: () -> any, settled: boolean?): Node
	return Receipt {
		dump = dump,
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
		settled = settled,
		text_size = if settled then nil else "$font.size.lg",
		line_height = if settled then nil else "$font.line.lg",
	}
end

-- How a dump stands, as Zeron's status says it.
local DUMP_STATUS: { [string]: string } = { routing = "working", routed = "done", needs_you = "input", failed = "failed" }

-- An earlier dump in the history: when, how it stands, its first words,
-- where its parts went (their hues, five at most and how many more); the
-- whole of what was said on the pointer; opened, its receipt under it.
local function history_row(_d: any, _i: number, row: State<any>): Node
	local function id(): string
		return row:peek().id
	end
	local function open(): boolean
		return history_opened:get()[row:get().id] == true
	end
	local hues = ui.computed(function(): { { key: string, hue: number? } }
		local out = {}
		local seen: { [string]: boolean } = {}
		for _, r in row:get().routes or {} do
			local t = data.target(orchestration.now:get(), chats_now:get(), r.target, function(k: string): string
				return k
			end)
			local key = `{t.kind}:{t.id or ""}`
			if not seen[key] then
				seen[key] = true
				table.insert(out, { key = key, hue = t.hue })
			end
		end
		return out
	end)
	return ui.column {
		class = "control-history-item",
		w_full = true,
		gap = "$space.2",
		ui.button {
			class = "control-history-row",
			ref = (function(): any
				local r = ui.ref()
				local key = row:peek().id
				history_refs[key] = r
				ui.on_cleanup(function()
					if history_refs[key] == r then
						history_refs[key] = nil
					end
				end)
				return r
			end)(),
			label = function(): string
				return row:get().text or ""
			end,
			expanded = open,
			tooltip = HoverCard.Tip { max_w = 360, p = "$space.2", ui.text {
				function(): string
					return row:get().text or ""
				end,
				literal = true,
				wrap = true,
				text_size = "$font.size.sm",
				color = "$text.primary",
			} },
			tooltip_delay = 500,
			w_full = true,
			h = 32,
			px = "$space.2",
			gap = "$space.2",
			items = "center",
			rounded = "$radius.md",
			bg = function(): string
				return if open() then "$hover" else "#00000000"
			end,
			hover = { bg = "$hover" },
			on_click = function()
				history_opened:set_key({ id() }, if open() then nil else true)
			end,
			status.StatusIcon {
				kind = function(): any
					return DUMP_STATUS[row:get().state or ""] or "waiting"
				end,
				size = 14,
			},
			ui.text {
				function(): string
					local t = string.gsub(row:get().text or "", "%s+", " ")
					return t
				end,
				literal = true,
				grow = 1,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.sm",
				color = "$text.primary",
			},
			-- Where its parts went.
			ui.each(ui.computed(function(): { any }
				local out = {}
				for i, h in hues:get() do
					if i <= 5 then
						table.insert(out, h)
					end
				end
				return out
			end), function(_h: any, _j: number, hh: State<any>): Node
				return hue.Dot { hue = function(): number?
					return hh:get().hue
				end, size = 6 }
			end, { key = function(h: any): string
				return h.key
			end, direction = "row", gap = "$zeron.space.hair", items = "center", shrink = 0 }),
			ui.text {
				function(): string
					local n = #hues:get()
					return if n > 5 then `+{n - 5}` else ""
				end,
				shrink = 0,
				tabular = true,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
			ui.text {
				function(): string
					return status.ago(status.since(row:get().created or 0))
				end,
				w = ui.raw(28, "a short time's room, so the rows' ends line up"),
				shrink = 0,
				tabular = true,
				align = "end",
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
		},
		ui.show(open, ui.column {
			class = "control-history-receipt",
			w_full = true,
			pl = ui.raw(30, "under the words, past the status"),
			pr = "$space.2",
			pb = "$space.2",
			receipt(function(): any
				return row:get()
			end, true),
		}),
	}
end

local function capture(): Node
	local newest = ui.computed(function(): any
		return orchestration.dumps:get()[1]
	end)
	local older = ui.computed(function(): { any }
		local out = {}
		for i, d in orchestration.dumps:get() do
			if i > 1 then
				table.insert(out, d)
			end
		end
		return out
	end)
	-- (All of it a drop zone for the composer's draft: a sidebar chat
	-- dropped is an @-mention, files are carried with the dump.)
	return LaneTab.DropZone { slot = "control-capture", chat = brain_dump, ui.column {
		id = "control-capture",
		w_full = true,
		gap = "$space.5",
		-- The chat composer (a side chat's: ds.Composer, wired), bound to the
		-- Brain dump's chat: files pasted or dropped, dictation, mentions,
		-- its model; Enter sends, as everywhere, each send a dump.
		LaneTab.Composer {
			id = "control-capture-composer",
			slot = "control-capture",
			chat = brain_dump,
			take = M.take,
			placeholder = locale.s("control.capture.placeholder"),
			-- (The Brain dump's model and agent; no access mode, no place:
			-- it routes, it works nowhere.)
			footer = { access = false, place = false },
		},
		ui.show(function(): boolean
			return newest:get() ~= nil
		end, ui.column {
			ref = newest_ref,
			w_full = true,
			gap = "$space.3",
			ui.text {
				function(): string
					local d = newest:get()
					return if d then status.ago(status.since(d.created or 0)) else ""
				end,
				tabular = true,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
			receipt(function(): any
				return newest:get()
			end),
		}),
		-- Earlier dumps, folded: a disclosure row, then a row each.
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
				h = 28,
				px = "$space.2",
				mx = ui.raw(-8, "its words over the rows' words, its wash out past them"),
				gap = "$space.2",
				items = "center",
				rounded = "$radius.md",
				bg = "#00000000",
				hover = { bg = "$hover" },
				self = "start",
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
				ui.text { locale.t("control.capture.history"), text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.secondary" },
				ui.text {
					function(): string
						return tostring(#older:get())
					end,
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
					y = Block.baseline("$font.size.sm", "$font.size.xs"),
				},
			},
			ui.show(function(): boolean
				return history_open:get()
			end, ui.column {
				w_full = true,
				mx = ui.raw(-8, "the rows' wash out past the block's words, their words in line"),
				ui.each(older, history_row, { key = function(d: any): string
					return d.id
				end, w_full = true, gap = "$zeron.space.hair" }),
			}),
		}),
	} }
end

-- A dump's receipt brought into view (a board card's source): the newest
-- where it stands, an earlier one opened in the history first. From
-- another page, `orchestration.reveal_dump` asks it as this one opens.
function M.show_dump(id: string?)
	if not id then
		return
	end
	local dumps = orchestration.now:peek().dumps or {}
	local first = dumps[1]
	local function focus(r: any): boolean
		local el = if r then r:peek() else nil
		return el ~= nil and el:focus() == true
	end
	if first and first.id == id then
		focus(newest_ref)
		return
	end
	ui.batch(function()
		history_open:set(true)
		history_opened:set_key({ id }, true)
	end)
	-- (Once the history's row is there.)
	ui.after(0, function()
		focus(history_refs[id])
	end)
end

-- The board: every orchestrator's work, its filter over it, loose items in
-- an Inbox first.
local function board(): Node
	return OrchBoard {
		id = "control-board",
		items = function(): { any }
			return orchestration.now:get().items or {}
		end,
		orchestrators = function(): { any }
			return orchestration.now:get().orchestrators or {}
		end,
		show_filter = true,
		inbox = true,
		todo_of = function(chat: string): any
			local c = chats_now:get()[chat]
			return c and c.todo
		end,
		thread_of = function(chat: string): any
			return chats_now:get()[chat]
		end,
		ask_of = function(chat: string): any
			return asks_now:get()[chat]
		end,
		-- (Whose a card is: the project its thread or orchestrator works in.)
		project_of = function(chat: string): string?
			local c = chats_now:get()[chat]
			local id = c and c.project
			if not id then
				return nil
			end
			for _, p in sessions.projects:get() or {} do
				if p.id == id then
					return p.name
				end
			end
			return nil
		end,
		on_move = function(id: string, state: string, order: number)
			orchestration.move_item(id, state, order):after(function() end, said("control.board.failed"))
		end,
		on_priority = function(id: string, p: string)
			orchestration.set_priority(id, p):after(function() end, said("control.board.failed"))
		end,
		on_open_thread = M.open_chat,
		on_source = function(src: any)
			M.show_dump(src.dump_id)
		end,
		on_remove = orchestration.remove_item,
		on_restore = orchestration.restore_item,
		on_start = orchestration.start_item,
		space_name = function(id: string): string?
			for _, p in sessions.projects:get() or {} do
				if p.id == id then
					return p.name
				end
			end
			return nil
		end,
		device_name = function(id: string): string?
			for _, d in sessions.devices:get() or {} do
				if d.id == id then
					return d.name
				end
			end
			return nil
		end,
		-- (Its clocks, from its lead thread's progress row.)
		progress_of = Progress.row,
		on_link = function(link: any)
			if link.kind == "chat" then
				M.open_chat(link.target)
			elseif string.match(link.target or "", "^https?://") then
				Links.open(link.target)
			end
		end,
	}
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

-- The Map's view (Topology, Timeline or Machines, and the Timeline's
-- span), on this device; what each thread did lately, as the host keeps it
-- (src/orchestration_timeline.rs).
M.map_view = ui.persist("control_map", OrchMap.DEFAULT)
local timeline = ui.shared("orchestration.timeline", {} :: { [string]: { any } })

local function map(): Node
	return ui.column {
		w_full = true,
		OrchMap.Map {
			world = world,
			view = M.map_view,
			timeline = function(): { [string]: { any } }
				return timeline:get()
			end,
			comms = function(): { any }
				return orchestration.comms:get()
			end,
			on_open_chat = M.open_chat,
			on_open_orchestrator = function(id: string)
				route.orchestrator(id)
			end,
			-- A secondary click on anything on it (screens/control/map_menu).
			menu = MapMenu.entries,
		},
		MapMenu.Dialogs(),
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
		name_of = thread_name,
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

-- Q&A (ds/orch_qa keeps its entries' model and what was read and put away).

local qa_entries = ui.computed(function(): { QA.Entry }
	return QA.entries(orchestration.dumps:get(), QA.marks:get(), chats_now:get())
end)

local function qa(): Node
	return QA.Section {
		entries = function(): { QA.Entry }
			return qa_entries:get()
		end,
		target = function(r: any): any
			return data.target(orchestration.now:get(), chats_now:get(), r.target, function(k: string): string
				return locale.s(`ds.orch.target.{k}`)
			end)
		end,
		marks = QA.marks,
		open = QA.opened,
		all = QA.all,
		on_open_thread = M.open_chat,
	}
end

local BUILD: { [string]: () -> Node } = { capture = capture, needs = needs, qa = qa, progress = Progress.Widget, map = map, board = board, machines = machines, comms = comms }

-- How many of the crew work now.
local function working_now(): number
	local working = 0
	for _, m in orchestration.now:get().crew or {} do
		local c = chats_now:get()[m.chat_id]
		if not m.released and c and c.status == "working" then
			working += 1
		end
	end
	return working
end

-- What the page's head counts, from what its blocks show, so the two never
-- disagree: the orchestrators (each one of yours, not archived; the Brain
-- dump isn't one), those working (the Progress block's count), those that
-- need you (Needs you's list; `urgent`, those of it that are).
local tally = {}
function tally.orchestrators(): number
	local n = 0
	for _, o in orchestration.now:get().orchestrators or {} do
		if (o.kind == nil or o.kind == "standard") and not o.archived then
			n += 1
		end
	end
	return n
end
function tally.working(): number
	local w = Progress.counts()
	return w
end
-- What waits on the person (data.waiting_for_you): Needs you's rows alone
-- (`needs`), or with the Q&A answers not read yet (`waiting`).
local function waiting(): data.Waiting
	return data.waiting_for_you(orchestration.now:get(), sessions.now:get(), QA.counts(qa_entries:get()))
end
function tally.needs(urgent: boolean?): number
	local w = waiting()
	return if urgent then w.urgent else #w.needs
end
function tally.waiting(): number
	return waiting().count
end
M.tally = tally

-- A figure that shows only while it has something to say.
local function figure_if(text: () -> string, tone: string?): Node
	return ui.show(function(): boolean
		return text() ~= ""
	end, Block.figure(text, tone))
end

-- The board's items by column today, counted.
local function board_counts(): { [string]: number }
	local out: { [string]: number } = {}
	-- (Done today as the board's Done column has it: since local midnight.)
	local now = os.time()
	for _, it in orchestration.now:get().items or {} do
		if data.shown_today(it, now) then
			out[it.state] = (out[it.state] or 0) + 1
		end
	end
	return out
end

-- What a widget's head says beside its title: its live figures.
local function figures_of(id: string): Node?
	if id == "needs" then
		local n = tally.needs
		return ui.row {
			gap = "$space.2",
			items = "center",
			ui.show(function(): boolean
				return n(false) > 0
			end, Counter { value = function(): number
				return n(false)
			end, text_size = "$font.size.xs", color = "$text.secondary" }),
			figure_if(function(): string
				local u = n(true)
				return if u > 0 then locale.s("control.figures.urgent", { n = u }) else ""
			end, "failed"),
		}
	elseif id == "qa" then
		local function counts(): { unread: number, waiting: number }
			return QA.counts(qa_entries:get())
		end
		return ui.row {
			gap = "$space.3",
			items = "center",
			-- (New answers in the unread dot's ink.)
			ui.show(function(): boolean
				return counts().unread > 0
			end, ui.text {
				function(): string
					return locale.s("control.figures.new", { n = counts().unread })
				end,
				class = "control-qa-new",
				tabular = true,
				shrink = 0,
				text_size = "$font.size.xs",
				weight = "$font.weight.medium",
				color = "$accent",
			}),
			figure_if(function(): string
				local n = counts().waiting
				return if n > 0 then locale.s("control.figures.waiting", { n = n }) else ""
			end),
		}
	elseif id == "map" then
		return figure_if(function(): string
			local w = working_now()
			return if w > 0 then locale.s("control.map.working", { n = w }) else ""
		end)
	elseif id == "board" then
		local function count(state: string, key: string): () -> string
			return function(): string
				local n = board_counts()[state] or 0
				return if n > 0 then locale.s(key, { n = n }) else ""
			end
		end
		return ui.row {
			gap = "$space.3",
			items = "center",
			figure_if(count("active", "control.figures.now")),
			figure_if(count("blocked", "control.figures.blocked"), "failed"),
			figure_if(count("review", "control.figures.review"), "input"),
			figure_if(count("done", "control.figures.done"), "done"),
		}
	elseif id == "machines" then
		return ui.row {
			gap = "$space.3",
			items = "center",
			figure_if(function(): string
				local n = 0
				for _, d in sessions.devices:get() or {} do
					if d.online ~= false then
						n += 1
					end
				end
				return if n > 0 then locale.s("control.figures.online", { n = n }) else ""
			end),
			figure_if(function(): string
				local n = 0
				for _, c in chats_now:get() do
					if c.status == "working" then
						n += 1
					end
				end
				return if n > 0 then locale.s("control.figures.running", { n = n }) else ""
			end),
		}
	elseif id == "progress" then
		return Progress.figures()
	elseif id == "comms" then
		return figure_if(function(): string
			local list = orchestration.comms:get()
			local last = list[#list]
			return if last then status.ago(status.since(last.at or 0)) else ""
		end)
	end
	return nil
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
		-- (Not while it has no box: ds/context_menu keeps the point whole.)
		local el = at
		if opener and el and el.mounted and el.x and el.y and (el.height or 0) > 0 then
			(opener :: any)(el.x, el.y + (el.height :: number) + 4)
		end
	end
end

-- One widget, a block of its own: its title, its live figures beside it,
-- its menu at its end; it over them.
local function widget(id: string, wide: boolean): Node
	local hook, seen, open = opens_under()
	local function entries(): { any }
		-- (The Map's own first: which view.)
		local out = if id == "map" then OrchMap.menu(M.map_view) else {}
		for _, e in {
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
		} do
			table.insert(out, e)
		end
		return out
	end
	return Block.Block {
		id = `control-widget-{id}`,
		class = "control-widget",
		title = locale.t(`control.widget.{id}`),
		figures = figures_of(id),
		menu = ui.div {
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
		head = function(row: Node): Node
			return ContextMenu {
				items = entries,
				hook = hook,
				label = locale.t(`control.widget.{id}`),
				w_full = true,
				row,
			}
		end,
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
		ui.slot("zeron.orchestration.widgets", { direction = "column", gap = GAP, w_full = true }),
	}
end

local function layout(kind: string): Node
	local wide = kind == "wide"
	local function list(column: string?): Node
		return ui.each(shown_of(column), function(w: { id: string }): Node
			return widget(w.id, wide)
		end, { key = by_id, w_full = true, gap = GAP })
	end
	if wide then
		return ui.column {
			id = "control-columns",
			class = "control-wide",
			w_full = true,
			gap = GAP,
			ui.row {
				w_full = true,
				gap = GAP,
				items = "start",
				ui.column { id = "control-left", grow = 1, basis = 0, min_w = 0, gap = GAP, list("left") },
				ui.column { id = "control-right", w = SIDE_W, shrink = 0, gap = GAP, list("right"), slot() },
			},
			ui.column { id = "control-full", w_full = true, gap = GAP, list("full") },
		}
	end
	return ui.column {
		id = "control-columns",
		class = "control-narrow",
		w_full = true,
		gap = GAP,
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
			-- What's going on: the counts the blocks show, set apart by room,
			-- not a glyph; those that need you in their own tone when any do.
			ui.row {
				id = "control-summary",
				gap = "$space.3",
				items = "center",
				min_w = 0,
				-- (On the title's baseline.)
				y = Block.baseline("$font.size.lg", "$font.size.sm"),
				ui.text {
					function(): string
						return locale.s("control.head.orchestrators", { n = tally.orchestrators() })
					end,
					tabular = true,
					text_size = "$font.size.sm",
					color = "$text.tertiary",
				},
				ui.text {
					function(): string
						return locale.s("control.head.working", { n = tally.working() })
					end,
					tabular = true,
					text_size = "$font.size.sm",
					color = function(): string
						return if tally.working() > 0 then "$text.secondary" else "$text.tertiary"
					end,
				},
				ui.text {
					function(): string
						return locale.s("control.head.needs", { n = tally.waiting() })
					end,
					tabular = true,
					text_size = "$font.size.sm",
					color = function(): string
						return if tally.waiting() > 0 then "$zeron.status.input" else "$text.tertiary"
					end,
				},
			},
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
	-- A dump asked to be shown from another page (a board card's source).
	ui.effect(function()
		local id = orchestration.reveal_dump:get()
		if id then
			ui.after(0, function()
				orchestration.reveal_dump:set(nil)
				M.show_dump(id)
			end)
		end
	end)
	-- Loads are sampled only while the page shows (Machines, the Map's lanes).
	orchestration.loads_wanted(true)
	ui.on_cleanup(function()
		orchestration.loads_wanted(false)
	end)
	local kept = KeptScroll.use(M.place)
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
			offset = kept.offset,
			on_wheel = kept.on_wheel,
			on_press = kept.on_press,
			on_key = kept.on_key,
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
					kept.on_resize(size, nil)
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

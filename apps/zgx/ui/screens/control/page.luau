-- The Control room (`/control`): the Brain dump's home, and the one place
-- that shows everything orchestration is doing. Three tabs (`control_tab`,
-- kept on this device; ⌘⌥1–3):
--
-- Overview: its widgets, in order: Capture (dump anything; the newest
-- dump's receipt under it, older ones folded), Needs you, Q&A (the
-- person's questions and their answers), Progress, the Map, the Scratchpad
-- (the person's notes and drawings, opened to a page of its own in place),
-- Machines and Comms. Two columns from 1100 points (Capture, the Map and
-- the Scratchpad on the left; the rest on the right), one below that. Each
-- widget can be hidden and moved; that's this device's (`control_widgets`).
-- Plugins add widgets of their own at `slot:zeron.orchestration.widgets`.
--
-- Board: the Board, the page's whole width.
--
-- Chat: the Brain dump's chat as a chat reads (its transcript, the chat
-- view's own), watched only while the tab shows.
--
-- Only the tab shown is built. Capture's composer floats at the page's
-- foot on Board and Chat, and on Overview once Capture is scrolled away:
-- one composer, the same draft, chips and model everywhere.
--
--   Page()    the page, as the shell mounts it

local ds = require("@ds")
local stage = require("@ds/stage")
local Icon = require("@ds/icon")
local IconButton = require("@ds/icon_button").IconButton
local ContextMenu = require("@ds/context_menu")
local locale = require("@ds/locale")
local status = require("@ds/status")
local hue = require("@ds/orch_hue")
local data = require("@ds/orch_data")
local Receipt = require("@ds/orch_receipt")
local decide = require("@ds/orch_decide")
-- (Loaded at their first use, not with the page: each takes a few
-- milliseconds to load, and the page is required inside the build that
-- opens it, whose time budget all of them together nearly spent.)
local function OrchMap(): any
	return require("@ds/orch_map")
end
local rows = require("@account/rows")
local function MapMenu(): any
	return require("@app/screens/control/map_menu")
end
local Machines = require("@ds/orch_machines")
local Visibility = require("@ds/orch_map_visibility")
local Name = require("@ds/orch_name")
local Comms = require("@ds/orch_comms")
local function OrchBoard(): any
	return require("@ds/orch_board")
end
local QA = require("@ds/orch_qa")
local Links = require("@ds/links")
local Block = require("@ds/orch_block")
local HoverCard = require("@ds/hover_card")
local Counter = require("@ds/orch_counter")
local Progress = require("@app/screens/control/progress")
local NeedsYou = require("@app/screens/control/needs")
local Decision = require("@ds/orch_decision")
local orchestration = require("@account/orchestration")
local sessions = require("@account/sessions")
local route = require("@app/screens/shell/route")
local function LaneTab(): any
	return require("@app/screens/chat/lane_tab")
end
local lanes = require("@app/screens/chat/lanes")
local KeptScroll = require("@ds/kept_scroll")
local hold = require("@ds/hold")
local prefs = require("@app/screens/settings/prefs")
local function Scratch(): any
	return require("@ds/orch_scratchpad")
end
local scratchpad = require("@account/scratchpad")
local Shared = require("@app/screens/chat/shared")
local motion = require("@ds/motion")
local material = require("@ds/material")
local glass = require("@ds/glass")

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
M.WIDGETS = { "capture", "needs", "qa", "progress", "map", "scratchpad", "machines", "comms" }
-- (The Board is a tab of its own: its columns want the page.)
local COLUMN: { [string]: string } = { capture = "left", map = "left", scratchpad = "left", needs = "right", qa = "right", progress = "right", machines = "right", comms = "right" }

-- The tabs, and the one shown, kept on this device.
M.TABS = { "overview", "board", "chat" }
M.tab = ui.persist("control_tab", "overview")
function M.show_tab(id: string)
	if table.find(M.TABS, id) and M.tab:peek() ~= id then
		M.tab:set(id)
	end
end

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
-- How many earlier dumps the history shows: the latest few, more on
-- "n more" (rows past these aren't built).
M.HISTORY_STEP = 10
local history_shown = ui.state("control_history_shown", M.HISTORY_STEP)

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

-- The tab whose readers are live (ds/hold): the one shown, a frame after
-- it's shown. The click's frame only shows what was there; what came
-- while it was hidden is taken in on the next.
M.awake = ui.state("control_tab_awake", "")
-- Whether a tab's readers are live.
function M.live(tab: string): () -> boolean
	return function(): boolean
		return M.awake:get() == tab
	end
end
-- Whether a tab is being built in the background (not shown): read once,
-- as it's built (never followed: that would build it again).
local function background(tab: string): boolean
	return M.tab:peek() ~= tab
end
local live_overview, live_board = M.live("overview"), M.live("board")
-- (Needs you floating on Board and Chat: live on either.)
local function live_float(): boolean
	local t = M.awake:get()
	return t == "board" or t == "chat"
end
-- What Overview and the Board read (`readers`, below): live while their tab
-- shows, held at their last value while it's kept hidden.
local OV: any, BD: any, NF: any

-- The chats by id, and the Q&A's entries: computeds the page makes again
-- each time it is built (`fresh`, from M.Page). The shell requires this
-- module while it builds the screen (screens/shell/boundary), so one made
-- here at the top would belong to that first build: gone with it, it would
-- keep its last value, and the page built again (a thread opened and come
-- back from) would show that forever.
local function chats_computed(): any
	return ui.computed(function(): { [string]: any }
		return data.chats(sessions.now:get())
	end)
end
local chats_now = chats_computed()
-- The open asks, by the chat that asked: a card whose thread asked waits on
-- the person. (Made again with each Page, as `chats_now` is.)
local function asks_computed(): any
	return ui.computed(function(): { [string]: any }
		return data.open_asks(orchestration.now:get())
	end)
end
local asks_now = asks_computed()

local function world(): any
	return {
		now = OV.now(),
		chats = OV.chats(),
		loaded = OV.loaded(),
		devices = OV.devices(),
		projects = OV.projects(),
		loads = OV.loads(),
		here = OV.here(),
		-- The Map's "Show chats", and the minute it's judged at
		-- (ds/orch_map_visibility).
		show = OV.vis(),
		clock = OV.clock(),
		-- (When this device read each chat, and what's been put away from
		-- the Map by hand: ds/orch_map_visibility.)
		read_at = sessions.read_at:get(),
		dismissed = Visibility.dismissed:get(),
	}
end

-- A board card shown (the Map's hover card): the Board tab, the card ringed
-- a moment where it stands.
M.board_reveal = ui.shared("control_board_reveal", nil :: string?)
function M.open_card(item: string)
	M.tab:set("board")
	M.board_reveal:set(item)
	ui.after(2400, function()
		if M.board_reveal:peek() == item then
			M.board_reveal:set(nil)
		end
	end)
end

-- Mark done as read, from the Map's menu: put away at once, an Undo for a
-- few seconds, then read on every device (sessions.seen, as opening each
-- would; a read can't be taken back, so it waits for the Undo to pass).
local READ_UNDO_MS = 6000
local read_batch = 0
local read_waiting: { [number]: { string } } = {}
local function put_back(ids: { string })
	local d = table.clone(Visibility.dismissed:peek() or {})
	for _, id in ids do
		d[id] = nil
	end
	Visibility.dismissed:set(d :: any)
end
-- Undo (the toast's): back on the Map, nothing read.
function M.undo_read(batch: number?)
	local b = batch or read_batch
	local ids = read_waiting[b]
	if ids then
		read_waiting[b] = nil
		put_back(ids)
	end
end
ds.toast.action("control.unread", function(batch: any)
	M.undo_read(batch)
end)
function M.mark_done_read(ids: { string })
	if #ids == 0 then
		return
	end
	read_batch += 1
	local batch = read_batch
	local now = os.time()
	local d = table.clone(Visibility.dismissed:peek() or {})
	for _, id in ids do
		d[id] = now
	end
	Visibility.dismissed:set(d :: any)
	read_waiting[batch] = ids
	ds.toast.show {
		key = `control.read.{batch}`,
		title = locale.s("control.map_menu.marked_read", { count = #ids }),
		tone = "neutral",
		duration = READ_UNDO_MS,
		action = { label = locale.s("control.map_menu.undo"), run = "control.unread", value = batch },
	}
	ui.after(READ_UNDO_MS, function()
		local waiting = read_waiting[batch]
		if waiting then
			read_waiting[batch] = nil
			for _, id in waiting do
				sessions.seen(id)
			end
		end
	end)
end

local function name_of(id: string?): string?
	return data.name_of(orchestration.now:get(), chats_now:get(), id)
end
-- A thread's name as the Map and the sidebar give it (ds/orch_name).
-- (Made again with each Page, as `chats_now` is.)
local function named_computed(): any
	return ui.computed(function(): any
		return Name.index(orchestration.now:get())
	end)
end
local named = named_computed()
local function thread_name(id: string): string
	return Name.from(named:get(), chats_now:get(), id)
end
local function hue_of(id: string?): number?
	return data.hue_of(orchestration.now:get(), chats_now:get(), id)
end

-- A chat opened: an orchestrator's on its own page, any other as a session;
-- at `message` when given (screens/control/needs).
M.open_chat = NeedsYou.open_chat

-- (Where only a chat's id is given.)
local function open_chat(id: string)
	M.open_chat(id)
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
-- dump's model as the composer has it picked, a dump; what it references
-- (the composer's `{ kind = "ref", ref }`, dragged from the room's widgets)
-- goes as the dump's `refs`, never as files.
function M.take(text: string, attachments: { any }, picks: { [string]: any }?): any
	local files, refs = {}, {}
	for _, a in attachments or {} do
		if a.kind == "ref" and type(a.ref) == "table" then
			table.insert(refs, a.ref)
		else
			table.insert(files, a)
		end
	end
	return orchestration.dump(text, files, brain_dump(), picks, refs)
end

-- A dump's receipt (ds/orch_receipt): the newest one's, or an earlier
-- one's opened in the history (`settled`: its parts there at once).
local function receipt(dump: () -> any, settled: boolean?): Node
	return Receipt {
		dump = dump,
		now = function(): any
			return OV.now()
		end,
		chats = function(): { [string]: any }
			return OV.chats()
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
	-- (The same table while its hues are: a push that changes no target of
	-- this row's wakes none of its dots.)
	local last_hues: any = nil
	local hues = ui.computed(function(): { { key: string, hue: number? } }
		local out = {}
		local seen: { [string]: boolean } = {}
		for _, r in row:get().routes or {} do
			local t = data.target(OV.now(), OV.chats(), r.target, function(k: string): string
				return k
			end)
			local key = `{t.kind}:{t.id or ""}`
			if not seen[key] then
				seen[key] = true
				table.insert(out, { key = key, hue = t.hue })
			end
		end
		if last_hues and same(out, last_hues) then
			return last_hues
		end
		last_hues = out
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

-- Capture's composer, in one of two places: in Capture, or, with Capture
-- scrolled up out of view, floating at the page's foot (bare: one line, a
-- count of what it carries; opened out on a click: its chips and model).
-- One composer: the same draft, chips and model in both (LaneTab's
-- `mirror`), the keys going with it as it changes places.
local SLOT = "control-capture"
local FOOTER: any = { access = false, place = false }
-- Capture's composer is out of the page's view, above it: it floats.
M.floating = ui.state("control_capture_floating", false)
-- The floating one opened out.
M.float_open = ui.state("control_capture_float_open", false)
-- (Whether either placement holds the keys, followed as focus moves.)
local inline_focused = false
local float_focused = false
-- The floating one's height as laid out (the Chat tab keeps its foot clear).
M.float_h = ui.state("control_capture_float_h", 0)
-- Asked for the keys: bumped, the floating one takes them.
local float_wanted = ui.state("control_capture_float_wanted", 0)
-- The scroll's own element, to tell above from below.
local scroll_ref = ui.ref()
-- The floating one's own element, to let go of the keys before it goes.
local float_ref = ui.ref()
-- (Capture's keys, to hand to the floating one as it comes.)
local float_takes_keys = false
local function inline_seen(visible: boolean, el: Element)
	local above = false
	if not visible then
		local view = scroll_ref:peek()
		local top = if view then view.y else nil
		above = el.y ~= nil and top ~= nil and (el.y :: number) < (top :: number)
	end
	if M.floating:peek() == above then
		return
	end
	if above then
		float_takes_keys = inline_focused
		M.floating:set(true)
		return
	end
	-- Going: the keys back into Capture's, let go here first (a focused
	-- node taken away hands the focus on, the view scrolling after it).
	local had = float_focused
	if had then
		local f = float_ref:peek()
		if f then
			f:blur()
		end
		float_focused = false
	end
	ui.batch(function()
		M.floating:set(false)
		if M.float_open:peek() then
			M.float_open:set(false)
		end
	end)
	if had then
		ui.after(0, function()
			LaneTab().focus(SLOT)
		end)
	end
end

-- The composer takes the keys where it shows: floating, else in Capture.
function M.focus_composer(): boolean
	if M.floating:peek() or M.tab:peek() ~= "overview" then
		float_wanted:add(1)
	else
		LaneTab().focus(SLOT)
	end
	return true
end

local function capture(): Node
	-- (Gone, hidden or laid out again: nothing of it floats till it says.)
	ui.on_cleanup(function()
		if M.floating:peek() then
			M.floating:set(false)
		end
	end)
	local newest = ui.computed(function(): any
		return OV.dumps()[1]
	end)
	-- The earlier ones: how many, and the latest `history_shown` of them,
	-- the rows the history builds.
	local older_n = ui.computed(function(): number
		return math.max(0, #OV.dumps() - 1)
	end)
	local older = ui.computed(function(): { any }
		local out = {}
		local n = history_shown:get()
		for i, d in OV.dumps() do
			if i > n + 1 then
				break
			end
			if i > 1 then
				table.insert(out, d)
			end
		end
		return out
	end)
	-- (All of it a drop zone for the composer's draft: a sidebar chat
	-- dropped is an @-mention, files are carried with the dump.)
	-- (And a card, thread, machine, ask… dragged from the room's widgets: a
	-- reference, ds/orch_ref.)
	return LaneTab().DropZone { slot = "control-capture", chat = brain_dump, refs = true, ui.column {
		id = "control-capture",
		w_full = true,
		gap = "$space.5",
		-- The chat composer (a side chat's: ds.Composer, wired), bound to the
		-- Brain dump's chat: files pasted or dropped, dictation, mentions,
		-- its model; Enter sends, as everywhere, each send a dump.
		-- (Watched: scrolled up out of the page's view, it floats at the
		-- page's foot instead, `floating`. Its focus is followed, so the keys
		-- go with it from one place to the other.)
		ui.column {
			id = "control-capture-inline",
			w_full = true,
			on_visible = inline_seen,
			LaneTab().Composer {
				id = "control-capture-composer",
				slot = SLOT,
				chat = brain_dump,
				take = M.take,
				-- (The Brain dump's chat watched only once it's used: its
				-- transcript, hundreds of routing turns, is never shown here.)
				lazy = true,
				-- (And then its row alone: never its transcript.)
				session_only = true,
				placeholder = locale.s("control.capture.placeholder"),
				-- (The Brain dump's model and agent; no access mode, no place:
				-- it routes, it works nowhere.)
				footer = FOOTER,
				on_focus = function(on: boolean)
					inline_focused = on
				end,
			},
		},
		-- (Under the composer, a frame after it: the newest's receipt, then
		-- the history, each a frame of its own, ds/stage.)
		stage.Later(1, function(): Node
			return ui.show(function(): boolean
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
			})
		end, if orchestration.dumps:peek()[1] then 120 else 0),
		-- Earlier dumps, folded: a disclosure row, then a row each.
		stage.Later(2, function(): Node
			return ui.show(function(): boolean
				return older_n:get() > 0
			end, ui.column {
				w_full = true,
				gap = "$space.1",
				ui.button {
					id = "control-history-toggle",
					label = function(): string
						return locale.s("control.capture.earlier", { n = older_n:get() })
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
							return tostring(older_n:get())
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
					-- Past the latest few: more of them, on a click.
					ui.show(function(): boolean
						return older_n:get() > history_shown:get()
					end, ui.button {
						id = "control-history-more",
						label = function(): string
							return locale.s("ds.orch.more", { n = older_n:get() - history_shown:get() })
						end,
						h = 28,
						px = "$space.2",
						items = "center",
						rounded = "$radius.md",
						bg = "#00000000",
						hover = { bg = "$hover" },
						self = "start",
						on_click = function()
							history_shown:set(history_shown:peek() + M.HISTORY_STEP)
						end,
						ui.text {
							function(): string
								return locale.s("ds.orch.more", { n = older_n:get() - history_shown:get() })
							end,
							text_size = "$font.size.sm",
							color = "$text.secondary",
						},
					}),
				}),
			})
		end, if orchestration.dumps:peek()[2] then 28 else 0),
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
	-- (Built a frame or more after the page, ds/stage: tried again till
	-- it's there, a second at most.)
	local function focus_soon(get: () -> any, tries: number)
		if focus(get()) or tries <= 0 then
			return
		end
		ui.after(50, function()
			focus_soon(get, tries - 1)
		end)
	end
	if first and first.id == id then
		focus_soon(function(): any
			return newest_ref
		end, 20)
		return
	end
	-- (Past the rows shown: as many more as reach it.)
	local at = 0
	for i, d in dumps do
		if d.id == id then
			at = i - 1
			break
		end
	end
	ui.batch(function()
		history_open:set(true)
		history_opened:set_key({ id }, true)
		if at > history_shown:peek() then
			history_shown:set(math.ceil(at / M.HISTORY_STEP) * M.HISTORY_STEP)
		end
	end)
	-- (Once the history's row is there.)
	ui.after(0, function()
		focus_soon(function(): any
			return history_refs[id]
		end, 20)
	end)
end

-- A link of a card's or an ask's: a chat opened here, a web one outside.
local function open_link(link: any)
	if link.kind == "chat" then
		M.open_chat(link.target)
	elseif string.match(link.target or "", "^https?://") then
		Links.open(link.target)
	end
end

-- The board: every orchestrator's work, its filter over it, loose items in
-- an Inbox first.
local function board(card_build: ((make: () -> Node, i: number) -> Node)?): Node
	return OrchBoard() {
		card_build = card_build,
		id = "control-board",
		items = function(): { any }
			return BD.now().items or {}
		end,
		orchestrators = function(): { any }
			return BD.now().orchestrators or {}
		end,
		show_filter = true,
		-- (Each card's thread, ask and owner read by themselves,
		-- account/rows through the Board's held readers: a push wakes the
		-- cards whose rows it changed, and none while the Board is hidden.)
		orchestrator_of = function(id: string): any
			return BD.orchestrator(id)
		end,
		todo_of = function(chat: string): any
			local c = BD.chat(chat)
			return c and c.todo
		end,
		thread_of = function(chat: string): any
			return BD.chat(chat)
		end,
		ask_of = function(chat: string): any
			return BD.ask(chat)
		end,
		-- (Whose a card is: the project its thread or orchestrator works in.)
		project_of = function(chat: string): string?
			local c = BD.chat(chat)
			local id = c and c.project
			if not id then
				return nil
			end
			for _, p in BD.projects() or {} do
				if p.id == id then
					return p.name
				end
			end
			return nil
		end,
		on_move = function(id: string, state: string, order: number)
			orchestration.move_item(id, state, order):after(function() end, said("control.board.failed"))
		end,
		-- (Put in Blocked: paused; a paused card taken out: resumed there.)
		-- (Their promises: refused, the board puts the card back and says so.)
		on_pause = orchestration.pause_item,
		on_resume = orchestration.resume_item,
		paused_open = M.board_paused,
		-- (An open ask naming a card marks it asked.)
		asks = function(): any
			local now = BD.now()
			return now and now.asks
		end,
		on_approve = function(id: string)
			orchestration.approve_item(id):after(function() end, said("control.board.failed"))
		end,
		on_request_changes = function(id: string, text: string)
			orchestration.request_changes(id, text):after(function() end, said("control.board.failed"))
		end,
		on_priority = function(id: string, p: string)
			orchestration.set_priority(id, p):after(function() end, said("control.board.failed"))
		end,
		on_open_thread = M.open_chat,
		-- (Its dump in Capture's history, on Overview.)
		on_source = function(src: any)
			if M.tab:peek() == "overview" then
				M.show_dump(src.dump_id)
				return
			end
			M.show_tab("overview")
			ui.after(0, function()
				M.show_dump(src.dump_id)
			end)
		end,
		on_remove = orchestration.remove_item,
		on_restore = orchestration.restore_item,
		expanded = M.board_expanded,
		later_open = M.board_later,
		on_start = orchestration.start_item,
		space_name = function(id: string): string?
			for _, p in BD.projects() or {} do
				if p.id == id then
					return p.name
				end
			end
			return nil
		end,
		device_name = function(id: string): string?
			for _, d in BD.devices() or {} do
				if d.id == id then
					return d.name
				end
			end
			return nil
		end,
		-- (Its clocks, from its lead thread's progress row.)
		progress_of = BD.row,
		on_link = open_link,
		-- A card waiting on a decision: its options as chips, and the
		-- whole of it in the overlay.
		on_answer = NeedsYou.on_answer,
		on_expand_ask = NeedsYou.on_expand,
	}
end

-- Needs you, every orchestrator's (screens/control/needs, as an
-- orchestrator's page has its own).
local function needs(): Node
	return NeedsYou.Asks {
		now = OV.now,
		sessions = OV.sessions,
		name_of = OV.name_of,
	}
end

-- The decision overlay (ds/orch_decision): the ask Needs you or a card
-- opened, whole.
local function decision(): Node
	return NeedsYou.Decision { name_of = name_of, hue_of = hue_of, on_link = open_link }
end

-- A Q&A entry, whole (ds/orch_decision's Answer, the decision's overlay):
-- opened from its row; a follow-up from it is a dump referencing the entry
-- (ds/orch_qa's ref), which the engine asks of the thread that answered.
local function qa_dialog(): Node
	return Decision.Answer {
		entry = function(): any
			return QA.shown_of(OV.qa())
		end,
		on_close = QA.hide,
		name_of = name_of,
		hue_of = hue_of,
		on_open = M.open_chat,
		on_source = function(entry: any)
			M.show_dump(entry.dump_id)
		end,
		on_read = function(entry: any, read: boolean)
			QA.mark(QA.marks, entry.key, read)
		end,
		on_dismiss = function(entry: any)
			QA.put_away(entry)
		end,
		on_follow_up = function(entry: any, text: string): any
			local sent: any = orchestration.dump(text, nil, brain_dump(), nil, { QA.ref(entry) })
			if sent ~= nil and (sent :: any).after ~= nil then
				(sent :: any):after(function()
					ds.toast.show { title = locale.s("ds.orch.qa.follow_up_sent", { name = name_of(entry.chat) or "" }) }
				end, said("ds.orch.qa.follow_up_failed"))
			end
			return sent
		end,
	}
end

-- The Map's view (Topology, Timeline or Machines, and the Timeline's
-- span), on this device; what each thread did lately, as the host keeps it
-- (src/orchestration_timeline.rs).
-- (ds/orch_map's DEFAULT, written out: the Map loads with its widget.)
M.MAP_DEFAULT = { variant = "force", window = 15 }
M.map_view = ui.persist("control_map", M.MAP_DEFAULT)
-- Whether the board's columns show all their cards (past ten): kept, as
-- the map's view is.
M.board_expanded = ui.persist("control_board_expanded", false)
-- Whether the board's Queued column shows its Later group: kept as well.
M.board_later = ui.persist("control_board_later", false)
-- Whether its Blocked column shows its Paused group: kept too.
M.board_paused = ui.persist("control_board_paused", false)
local timeline = ui.shared("orchestration.timeline", {} :: { [string]: { any } })

local function map(): Node
	return ui.column {
		w_full = true,
		OrchMap().Map {
			world = world,
			-- (Each thread's chat and name by itself, account/rows through
			-- the Overview's held readers: a push wakes the Map rows of the
			-- chats it changed, none while the Overview is hidden.)
			chat = function(id: string): any
				return OV.chat(id)
			end,
			name_of = function(id: string): string
				return OV.name(id)
			end,
			view = M.map_view,
			timeline = function(): { [string]: { any } }
				return OV.timeline()
			end,
			comms = function(): { any }
				return OV.comms()
			end,
			on_open_chat = open_chat,
			on_open_orchestrator = function(id: string)
				route.orchestrator(id)
			end,
			-- A secondary click on anything on it (screens/control/map_menu).
			menu = MapMenu().entries,
			mark_read = M.mark_done_read,
			-- (The hover card's: Progress's row of a chat, and a card shown.)
			progress = function(id: string): any
				return (orchestration.progress:get() or {})[id]
			end,
			on_open_card = M.open_card,
			on_resume_card = function(id: string)
				orchestration.resume_item(id):after(function() end, said("orch.board.resume_failed"))
			end,
		},
		MapMenu().Dialogs(),
	}
end

local function machines(): Node
	return Machines {
		devices = function(): { any }
			return OV.devices()
		end,
		loads = function(): { [string]: any }
			return OV.loads()
		end,
		chats = function(): { [string]: any }
			return OV.chats()
		end,
		here = function(): string?
			return OV.here()
		end,
		hue_of = OV.hue_of,
		name_of = OV.thread_name,
		on_open_chat = open_chat,
	}
end

local function comms(): Node
	return Comms {
		comms = function(): { any }
			return OV.comms()
		end,
		name_of = OV.name_of,
		hue_of = OV.hue_of,
		on_open = open_chat,
	}
end

-- Q&A (ds/orch_qa keeps its entries' model and what was read and put away).

local function qa_computed(): any
	return ui.computed(function(): { QA.Entry }
		return QA.entries(orchestration.now:get().dumps or {}, QA.marks:get(), chats_now:get())
	end)
end
local qa_entries = qa_computed()

-- A tab's readers of what the page shows, each held while the tab is
-- hidden (ds/hold, counted as `hold.<tab>`). (Read through `chats_now` and
-- the rest at each call: they're made again with each Page.)
local function readers(live: () -> boolean, name: string): any
	local function held(read: () -> any): () -> any
		local c = hold.computed(live, read, name)
		return function(): any
			return c:get()
		end
	end
	local R: any = { live = live }
	R.now = held(function(): any
		return orchestration.now:get()
	end)
	R.dumps = held(function(): any
		return orchestration.dumps:get()
	end)
	R.comms = held(function(): any
		return orchestration.comms:get()
	end)
	R.loads = held(function(): any
		return orchestration.loads:get()
	end)
	R.sessions = held(function(): any
		return sessions.now:get()
	end)
	R.devices = held(function(): any
		return sessions.devices:get()
	end)
	R.projects = held(function(): any
		return sessions.projects:get()
	end)
	R.here = held(function(): any
		return sessions.here:get()
	end)
	R.chats = held(function(): any
		return chats_now:get()
	end)
	-- (Both loaded from the engine: what's missing from them is gone, not
	-- yet to come.)
	R.loaded = held(function(): boolean
		return sessions.now:get().state == "ready" and (orchestration.now:get() or {}).state == "ready"
	end)
	R.asks = held(function(): any
		return asks_now:get()
	end)
	R.named = held(function(): any
		return named:get()
	end)
	-- One row by itself (account/rows), as held: while shown, a reader
	-- wakes only for its own row; hidden, its last value, nothing read.
	R.chat = hold.by(live, rows.chat, name)
	R.ask = hold.by(live, rows.ask, name)
	R.orchestrator = hold.by(live, rows.orchestrator, name)
	R.name = hold.by(live, rows.name, name)
	R.qa = held(function(): any
		return qa_entries:get()
	end)
	R.timeline = held(function(): any
		return timeline:get()
	end)
	R.pad = held(function(): any
		return scratchpad.now:get()
	end)
	R.vis = held(function(): any
		return Visibility.mode:get()
	end)
	R.clock = held(function(): any
		return Visibility.now()
	end)
	local rows = held(function(): any
		return Progress.rows:get()
	end)
	-- A chat's progress row (a board card's clocks).
	function R.row(id: string): any
		return rows()[id]
	end
	-- A name, a hue: from what's held.
	function R.name_of(id: string?): string?
		return data.name_of(R.now(), R.chats(), id)
	end
	function R.thread_name(id: string): string
		return Name.from(R.named(), R.chats(), id)
	end
	function R.hue_of(id: string?): number?
		return data.hue_of(R.now(), R.chats(), id)
	end
	return R
end


-- Made again for this build of the page (see `chats_computed`).
local function fresh()
	chats_now = chats_computed()
	asks_now = asks_computed()
	named = named_computed()
	qa_entries = qa_computed()
	OV = readers(live_overview, "overview")
	BD = readers(live_board, "board")
	NF = readers(live_float, "needs_float")
end

local function qa(): Node
	return QA.Section {
		entries = function(): { QA.Entry }
			return OV.qa()
		end,
		target = function(r: any): any
			return data.target(OV.now(), OV.chats(), r.target, function(k: string): string
				return locale.s(`ds.orch.target.{k}`)
			end)
		end,
		marks = QA.marks,
		open = QA.opened,
		all = QA.all,
		on_open_thread = open_chat,
	}
end

-- The Scratchpad: its widget, and its page (the widget opened, in place of
-- the others: `M.scratchpad_open`).
M.scratchpad_open = ui.state("control_scratchpad_open", false)
local function scratchpad_props(): any
	return {
		pad = function(): any
			return OV.pad()
		end,
		on_save_text = function(text: string): any
			return scratchpad.set_text(text)
		end,
		on_save_shapes = function(shapes: { any }): any
			return scratchpad.set_shapes(shapes)
		end,
		on_restore = function(id: string)
			scratchpad.restore(id):after(function() end, said("ds.orch.scratchpad.restore_failed"))
		end,
		on_expand = function()
			M.scratchpad_open:set(true)
		end,
		on_collapse = function()
			M.scratchpad_open:set(false)
		end,
		on_page_wheel = function(dy: number)
			KeptScroll.scroll_by(M.place, dy)
		end,
	}
end
local function scratchpad_widget(): Node
	return Scratch().Widget(scratchpad_props())
end

local BUILD: { [string]: () -> Node } = { scratchpad = scratchpad_widget, capture = capture, needs = needs, qa = qa, progress = function(): Node
	return Progress.Widget(live_overview)
end, map = map, machines = machines, comms = comms }

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
-- (An ask answered here, waiting out its Undo, on its way or taken, isn't
-- counted: it comes back if it's undone or turned down.)
local function waiting(): data.Waiting
	return data.waiting_for_you(orchestration.now:get(), sessions.now:get(), QA.counts(qa_entries:get()), decide.counted)
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
local function figure_if_live(text: () -> string, tone: string?): Node
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
local function figures_of(id: string, live: () -> boolean): Node?
	-- (Held while its tab is hidden, as what it counts is: ds/hold.)
	local function figure_if(text: () -> string, tone: string?): Node
		return figure_if_live(hold.fn(live, text), tone)
	end
	if id == "needs" then
		local n = hold.by(live, function(urgent: boolean): number
			return tally.needs(urgent)
		end)
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
		local counts = hold.fn(live, function(): { unread: number, waiting: number }
			return QA.counts(qa_entries:get())
		end)
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
		return Progress.figures(live)
	elseif id == "scratchpad" then
		return Scratch().Status()
	elseif id == "comms" then
		return figure_if(function(): string
			local list = OV.comms()
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

-- Opening the page, the first STAGE_NOW widgets (in its order) build with
-- it and the rest one a frame after (ds/stage), each holding a block's room
-- till then: built all at once, the Map, the Scratchpad, the Board and the
-- rest took 0.4 to 1.4 s in one frame (the window not answering meanwhile).
local STAGE_NOW = 2
-- (Built before it's opened, Overview hidden: in the background instead,
-- ds/stage's `Idle`, all at once should it be opened first.)
local function staged(id: string): Node
	local rank = table.find(full_order(M.widgets:peek()), id) or #M.WIDGETS
	if background("overview") then
		return stage.Idle { rank = rank, build = BUILD[id], h = 160, now = live_overview, group = "overview" }
	end
	return stage.Later(rank - STAGE_NOW, BUILD[id])
end

-- One widget, a block of its own: its title, its live figures beside it,
-- its menu at its end; it over them.
local function widget(id: string, wide: boolean): Node
	local hook, seen, open = opens_under()
	local function entries(): { any }
		-- (The Map's own first: which view.)
		local out = if id == "map" then OrchMap().menu(M.map_view) else {}
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
		figures = figures_of(id, live_overview),
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
		staged(id),
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

-- The widgets' menu: each shown or not, and back to how they came (the
-- head's menu, and the sidebar's view menu, "Widgets…").
function M.widget_entries(): { any }
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

-- The page's head: its name, what's going on, and the widgets' menu.
local function head(): Node
	local hook, seen, open = opens_under()
	return ContextMenu {
		-- (Overview's widgets: the other tabs have none.)
		items = function(): { any }
			return if M.tab:peek() == "overview" then M.widget_entries() else {}
		end,
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
			-- The widgets' menu: Overview's alone, to the tabs' left, so the
			-- tabs stand at the head's end, in the same place on every tab.
			ui.div {
				on_visible = seen,
				hidden = function(): boolean
					return M.tab:get() ~= "overview"
				end,
				IconButton {
					id = "control-widgets-menu",
					icon = "tuning",
					label = locale.t("control.widgets.title"),
					size = "md",
					tone = "quiet",
					on_click = open,
				},
			},
			-- The tabs.
			ds.Segmented {
				id = "control-tabs",
				value = M.tab,
				label = locale.t("control.tabs.label"),
				options = {
					{ value = "overview", label = locale.t("control.tabs.overview") },
					{ value = "board", label = locale.t("control.tabs.board") },
					{ value = "chat", label = locale.t("control.tabs.chat") },
				},
			},
		},
	}
end

-- The floating composer's widths: bare (one line), opened out.
local FLOAT_W, FLOAT_OPEN_W = 520, 680
-- Clear of the page's sides, and of its foot.
local FLOAT_SIDE, FLOAT_FOOT = 24, 20
-- Needs you floating (on Board and Chat): its width, and its list's
-- room before it scrolls.
local NEEDS_W = 380
local NEEDS_MAX_H = 460
-- Clear of a toast stack beside it.
local FLOAT_TOAST_GAP = 12
-- The companion's size: the pill's own one line (ds.Composer's ONE_LINE
-- plus its hairlines).
local FLOAT_KNOB = 51

-- Capture's composer floating at the page's foot (Capture scrolled up out
-- of view): bare, one line and beside it a knob (what it carries, counted;
-- a click opens it out), or opened out, its chips over the draft and its
-- model under it. Escape, or the knob, folds it back. Its own drop zone,
-- as Capture is one. `take_keys`: it takes them as it comes (they were in
-- Capture's).
-- Kept built while the page is (`shown`: whether it floats now), so going to
-- a tab where it floats only shows it.
local FloatingCapture = ui.component(function(props: { width: () -> number, shown: () -> boolean }): Node
	local open = M.float_open
	local focus = ui.use_state(0)
	-- (The keys asked for once its form is laid out: one made again, bare
	-- or opened out, takes them as it comes.)
	local alive = true
	local function take_keys()
		ui.after(0, function()
			ui.after(0, function()
				if alive then
					focus:add(1)
				end
			end)
		end)
	end
	-- Shown: the keys, if Capture's had them; hidden: let go of them, folded.
	local was_shown = props.shown()
	if was_shown and float_takes_keys then
		float_takes_keys = false
		take_keys()
	end
	ui.effect(function()
		local on = props.shown()
		if on == was_shown then
			return
		end
		was_shown = on
		if on then
			if float_takes_keys then
				float_takes_keys = false
				take_keys()
			end
			return
		end
		ui.after(0, function()
			if float_focused then
				local f = float_ref:peek()
				if f then
					f:blur()
				end
				float_focused = false
			end
			if open:peek() then
				open:set(false)
			end
		end)
	end)
	local wanted = float_wanted:peek()
	ui.effect(function()
		local n = float_wanted:get()
		if n ~= wanted then
			wanted = n
			take_keys()
		end
	end)
	ui.on_cleanup(function()
		alive = false
		float_focused = false
	end)
	local function set_open(on: boolean)
		if open:peek() ~= on then
			open:set(on)
		end
		take_keys()
	end
	local count = ui.computed(function(): number
		local chat = brain_dump()
		return if chat then #Shared.cargo_of(chat) else 0
	end)
	local w = ui.computed(function(): number
		local room = math.max(0, props.width() - FLOAT_SIDE * 2)
		return math.min(if open:get() then FLOAT_OPEN_W else FLOAT_W, room)
	end)
	-- (Where it stands in the window, as last laid out: a toast stack in
	-- the corner beside it lifts it over the stack.)
	local box = ui.use_state(nil :: { x: number, w: number }?)
	local lift = ui.computed(function(): number
		local h = ds.toast.height:get()
		local b = box:get()
		if h <= 0 or not b then
			return 0
		end
		local size = (ui :: any).window and (ui :: any).window.size
		local win = if size then size:get() else nil
		local win_w = if type(win) == "table" and type(win.width) == "number" then win.width else 0
		local toast_w = (ui.theme.get("zeron.toast.width") :: number?) or 340
		local left = win_w - ds.toast.RIGHT - toast_w - FLOAT_TOAST_GAP
		if win_w <= 1 or b.x + b.w <= left then
			return 0
		end
		return math.max(0, ds.toast.foot() + h + FLOAT_TOAST_GAP - FLOAT_FOOT)
	end)
	local knob = ui.button {
		id = "control-float-knob",
		label = locale.t("control.capture.expand"),
		w = ui.raw(FLOAT_KNOB, "the pill's one line, as round"),
		h = ui.raw(FLOAT_KNOB, "the pill's one line"),
		shrink = 0,
		gap = "$space.1",
		items = "center",
		justify = "center",
		rounded = "$radius.full",
		border = 1,
		border_color = "$zeron.composer.edge",
		bg = function(): string
			return if material.see_through() then "$zeron.composer.tint" else "$zeron.composer.solid"
		end,
		backdrop_blur = function(): number
			return if material.see_through() then 16 else 0
		end,
		shadow = function(): string
			return if material.see_through() then "$zeron.composer.rest" else "$zeron.composer.shadow"
		end,
		hover = { border_color = "$border.strong" },
		color = "$text.secondary",
		focus_on_press = false,
		on_click = function()
			set_open(true)
		end,
		ui.presence(function(): string
			return if count:get() > 0 then "count" else "open"
		end, function(kind: string): Node
			if kind == "count" then
				return ui.row {
					gap = ui.raw(2, "the clip and its number, one mark"),
					items = "center",
					Icon { name = "paperclip", size = 14 },
					ui.text {
						function(): string
							return tostring(count:get())
						end,
						class = "control-float-count",
						tabular = true,
						text_size = "$font.size.sm",
						weight = "$font.weight.medium",
						color = "$text.primary",
					},
				}
			end
			return Icon { name = "expand-arrows", size = 16 }
		end, {
			enter = motion.transition({ opacity = { 0, 1 }, scale = { 0.8, 1 }, duration = 160, ease = "out(3)" }),
			exit = motion.transition({ opacity = 0, duration = 100 }),
		}),
	}
	return ui.row {
		w_full = true,
		-- (Off the page's foot, and over a toast stack beside it.)
		pb = motion.tween(ui.computed(function(): number
			return FLOAT_FOOT + lift:get()
		end), { duration = 220, ease = "out(3)" }),
		justify = "center",
		pointer_events = "none",
		ui.column {
			id = "control-float",
			ref = float_ref,
			w = motion.tween(w, { duration = 220, ease = "out(3)" }),
			pointer_events = function(): any
				return if props.shown() then "auto" else "none"
			end,
			on_resize = function(size: { w: number, h: number }, el: Element)
				if math.abs(size.h - M.float_h:peek()) >= 1 then
					M.float_h:set(size.h)
				end
				local x, bw = el.x, el.width
				if x and bw then
					local b = box:peek()
					if not b or math.abs(b.x - x) >= 1 or math.abs(b.w - bw) >= 1 then
						box:set({ x = x, w = bw })
					end
				end
			end,
			LaneTab().DropZone {
				slot = SLOT,
				chat = brain_dump,
				focus = focus,
				place = "float",
				-- (References dragged from the room too, as Capture's own.)
				refs = true,
				ui.row {
					w_full = true,
					gap = "$space.2",
					items = "end",
					ui.column {
						id = "control-float-panel",
						grow = 1,
						min_w = 0,
						relative = true,
						-- Opened out, on a panel of the menus' glass (the page under
						-- it never reads through), the model's footer inside it.
						p = function(): any
							return if open:get() then "$space.3" else 0
						end,
						ui.show(function(): boolean
							return open:get()
						end, ui.div(glass.style({
							class = "control-float-surface",
							absolute = true,
							left = 0,
							right = 0,
							top = 0,
							bottom = 0,
							pointer_events = "none",
							rounded = "$radius.xl",
						}, "menu") :: any)),
						-- Its head: folding it back, at the right.
						ui.show(function(): boolean
							return open:get()
						end, ui.row {
							w_full = true,
							justify = "end",
							pb = "$space.2",
							IconButton {
								id = "control-float-collapse",
								icon = "collapse-arrows",
								label = locale.t("control.capture.collapse"),
								size = "md",
								tone = "quiet",
								on_click = function()
									set_open(false)
								end,
							},
						}),
						LaneTab().Composer {
							id = "control-float-composer",
							slot = SLOT,
							chat = brain_dump,
							take = M.take,
							lazy = true,
							session_only = true,
							placeholder = locale.s("control.capture.placeholder"),
							footer = FOOTER,
							mirror = true,
							focus = focus,
							bare = function(): boolean
								return not open:get()
							end,
							on_escape = function(): boolean
								if open:peek() then
									set_open(false)
									return true
								end
								return false
							end,
							on_open_model = function()
								set_open(true)
							end,
							on_focus = function(on: boolean)
								float_focused = on
							end,
							shown = props.shown,
						},
					},
					-- (Beside it, bare: opened out, its panel's head folds it.)
					ui.show(function(): boolean
						return not open:get()
					end, ui.div {
						shrink = 0,
						knob,
					}),
				},
			},
		},
	}
end)

-- The page's frame: as wide as the widgets go, centred, off its sides.
local MAX_W = 1480
-- (Clear of the floating composer, at a transcript's foot.)
local FLOAT_CLEAR = 12

-- A tab's scroll, its place kept (KeptScroll), its content centred.
local function scrolled(props: { id: string, place: any, ref: any?, pb: any?, [number]: Node }): Node
	local kept = KeptScroll.use(props.place)
	local body: { [any]: any } = {
		w_full = true,
		max_w = MAX_W,
		on_resize = function(size: { w: number, h: number })
			kept.on_resize(size, nil)
		end,
	}
	for _, child in ipairs(props :: any) do
		table.insert(body, child)
	end
	return ui.scroll {
		ref = props.ref,
		id = props.id,
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
			pt = "$space.2",
			pb = props.pb or "$space.8",
			ui.column(body :: any),
		},
	}
end

-- Overview: Capture and the widgets (or the Scratchpad opened to its page).
local Overview = ui.component(function(props: { width: State<number> }): Node
	-- Loads are sampled only while the widgets show (Machines, the Map's
	-- lanes): not while the tab is kept hidden.
	local wanted = false
	ui.effect(function()
		local on = live_overview()
		if on ~= wanted then
			wanted = on
			orchestration.loads_wanted(on)
		end
	end)
	ui.on_cleanup(function()
		if wanted then
			orchestration.loads_wanted(false)
		end
	end)
	return scrolled {
		id = "control-scroll",
		place = M.place,
		ref = scroll_ref,
		-- (A real swap: the widgets, or the Scratchpad opened to its page.)
		ui.presence(function(): string
			return if M.scratchpad_open:get() then "scratchpad" else "widgets"
		end, function(kind: string): Node
			if kind == "scratchpad" then
				return ui.column {
					w_full = true,
					Scratch().Page(scratchpad_props()),
				}
			end
			return ui.presence(function(): string
				-- (A real swap: two columns or one, the widgets laid out again.)
				return if props.width:get() >= M.WIDE then "wide" else "narrow"
			end, layout, { w_full = true })
		end, { w_full = true }),
	}
end)

-- Where the Board tab is scrolled, kept as Overview's is.
M.board_place = ui.state("control_board_scroll", 0)

-- Board: its figures, then the board across the page (a frame after the
-- tab's first, ds/stage), its cards a few a frame. Built before it's opened
-- (in the background, the Board hidden): a card a frame (ds/stage's
-- `Idle`), out of layout till they're all in.
local BoardTab = ui.component(function(): Node
	local idle = background("board")
	-- Its cards a few a frame, the top of each column first, never all in
	-- one: built in the background one a frame; the Board shown (opened on
	-- it, or before they're all in), `URGENT_PER_FRAME` a frame.
	local function shown(): boolean
		return M.tab:peek() == "board"
	end
	local function build(): Node
		return board(function(make: () -> Node, i: number): Node
			return stage.Idle { rank = i, build = make, h = 96, urgent = shown }
		end)
	end
	return scrolled {
		id = "control-board-scroll",
		place = M.board_place,
		-- (Its last cards scroll clear of the floating composer, and of
		-- Needs you floating over its right-hand columns.)
		pb = function(): any
			local room = math.max(M.float_h:get() + FLOAT_FOOT, M.needs_room:get())
			return ui.raw(math.max(32, room + FLOAT_CLEAR), "what floats at its foot, measured")
		end,
		ui.column {
			id = "control-board-tab",
			w_full = true,
			gap = "$space.3",
			ui.row { id = "control-board-figures", w_full = true, min_h = 20, items = "center", figures_of("board", live_board) },
			if idle then stage.Idle { rank = 0, build = build, h = 480, now = live_board, group = "board" } else stage.Later(1, build, 480),
		},
	}
end)

-- Chat: the Brain dump's chat, read as any chat is (screens/chat/lane_tab's
-- transcript, the chat view's), in a lane of its own watched while the tab
-- shows and let go as it goes. Its foot kept clear of the floating composer.
M.CHAT_SLOT = "control-chat"
local ChatTab = ui.component(function(): Node
	ui.on_cleanup(function()
		lanes.watch(M.CHAT_SLOT, nil)
	end)
	return ui.column {
		id = "control-chat",
		w_full = true,
		grow = 1,
		min_h = 0,
		ui.presence(function(): string
			return brain_dump() or ""
		end, function(id: string): Node
			if id == "" then
				return ds.EmptyState {
					icon = "chat-round-line",
					title = locale.t("control.chat.none") :: any,
					fill = true,
				}
			end
			local function build(): Node
				return LaneTab().Tab {
					tab = {
						id = M.CHAT_SLOT,
						kind = "read",
						chat = id,
						title = locale.s("control.tabs.chat"),
						data = {
							-- (Clear of the floating composer, and of Needs you
							-- where the page is narrow enough for it to reach
							-- the transcript.)
							foot = function(): number
								local room = M.float_h:get() + FLOAT_FOOT
								local w = M.page_w:get()
								local half = prefs.conversation_width:get() / 2 + 24
								if w > 0 and w / 2 + half > w - NEEDS_W - FLOAT_SIDE then
									room = math.max(room, M.needs_room:get())
								end
								return room + FLOAT_CLEAR
							end,
							live = M.live("chat"),
						},
					},
				}
			end
			-- (Built before it's opened: in the background.)
			if background("chat") then
				return stage.Idle { rank = 0, build = build, h = 0, now = M.live("chat"), group = "chat" }
			end
			return stage.Later(1, build)
		end, { w_full = true, grow = 1, min_h = 0 }),
	}
end)

-- Needs you, floating on Board and Chat (Overview has its widget): what
-- waits on the person, answerable where they are. Compact: its count and
-- the top ask (its chips answer it, as the widget's), the rest a click
-- away ("n more", the widget's own). At the page's foot on the right, clear
-- of the floating composer (lifted over it where the page is too narrow
-- for both). Built only while it shows; nothing waiting, nothing at all.
-- Minimised to its count (a pill): kept on this device, and opened again by
-- an ask it hasn't seen.
M.page_w = ui.state("control_page_w", 0)
M.needs_min = ui.persist("control_needs_min", false)
-- The room it takes at the page's foot (its height and its offset from the
-- foot), while it shows: what Board and Chat keep clear at their ends.
M.needs_room = ui.state("control_needs_room", 0)
local NeedsFloat = ui.component(function(props: { bottom: () -> number }): Node
	-- (Its list's own height, to the room it has: a scroll doesn't take its
	-- content's.)
	local content_h = ui.use_state(0)
	-- An ask it hasn't seen opens it again (one seen, answered, gone: no).
	local seen: { [string]: boolean }? = nil
	ui.effect(function()
		local now: { [string]: boolean } = {}
		local fresh = false
		for _, n in data.needs(NF.now(), NF.sessions()) do
			local key = tostring(n.id or n.chat_id or "")
			now[key] = true
			if seen and not seen[key] then
				fresh = true
			end
		end
		seen = now
		if fresh and M.needs_min:peek() then
			M.needs_min:set(false)
		end
	end)
	ui.on_cleanup(function()
		M.needs_room:set(0)
	end)
	local function opened(): Node
	return ui.column(glass.style({
		id = "control-needs-float",
		class = "control-needs-float",
		w = NEEDS_W,
		rounded = "$radius.xl",
		p = "$space.3",
		gap = "$space.2",
		pointer_events = "auto",
		ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			ui.text { locale.t("control.widget.needs"), text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.primary" },
			Counter { value = function(): number
				return tally.needs(false)
			end, text_size = "$font.size.xs", color = "$text.secondary" },
			ui.div { grow = 1 },
			-- Minimised to its count: out of the way, a click from back.
			IconButton {
				id = "control-needs-min",
				icon = "minus",
				label = locale.t("control.needs.minimise"),
				size = "sm",
				tone = "quiet",
				on_click = function()
					M.needs_min:set(true)
				end,
			},
		},
		ui.scroll {
			w_full = true,
			h = function(): any
				return ui.raw(math.min(content_h:get(), NEEDS_MAX_H), "its list's height, to its room")
			end,
			ui.column {
				w_full = true,
				on_resize = function(size: { w: number, h: number })
					if math.abs(size.h - content_h:peek()) >= 1 then
						content_h:set(size.h)
					end
				end,
			-- (Needs you's own rows: screens/control/needs. The top ask
			-- alone, no recent answers: the widget on Overview has them.)
			NeedsYou.Asks {
				now = NF.now,
				sessions = NF.sessions,
				name_of = NF.name_of,
				recent = false,
				limit = 1,
			},
			},
		},
	}, "menu") :: any)
	end
	-- Minimised: its count on a pill; a click opens it.
	local function pill(): Node
		return ui.button(glass.style({
			id = "control-needs-pill",
			class = "control-needs-pill",
			label = function(): string
				return locale.s("control.head.needs", { n = tally.needs(false) })
			end,
			h = ui.raw(36, "a pill, one line"),
			px = "$space.3",
			gap = "$space.2",
			items = "center",
			rounded = "$radius.full",
			pointer_events = "auto",
			on_click = function()
				M.needs_min:set(false)
			end,
			status.StatusIcon { kind = "input", size = 14 },
			ui.text {
				function(): string
					return locale.s("control.head.needs", { n = tally.needs(false) })
				end,
				tabular = true,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.primary",
			},
		}, "menu") :: any)
	end
	return ui.column {
		on_resize = function(size: { w: number, h: number })
			local room = size.h + props.bottom()
			if math.abs(room - M.needs_room:peek()) >= 1 then
				M.needs_room:set(room)
			end
		end,
		items = "end",
		ui.presence(function(): string
			return if M.needs_min:get() then "pill" else "open"
		end, function(k: string): Node
			return if k == "pill" then pill() else opened()
		end, {
			enter = motion.transition({ opacity = { 0, 1 }, scale = { 0.96, 1 }, duration = 160, ease = "out(3)" }),
		}),
	}
end)

-- The tabs built so far: each stays once built, hidden while another
-- shows. The rest are built in the background `PREBUILD_AFTER` ms after the
-- page opens (ds/stage's `Idle`: a few pieces a frame, while the window is
-- idle), so even a first click only shows what's there.
M.built = ui.state("control_tabs_built", {} :: { [string]: boolean })
M.PREBUILD_AFTER = 1200

-- One tab, kept: built once asked for, hidden while another shows.
local function kept(tab: string, build: () -> Node): Node
	return ui.presence(function(): string?
		return if M.built:get()[tab] then tab else nil
	end, function(): Node
		return ui.column {
			id = `control-tab-{tab}`,
			w_full = true,
			h_full = true,
			min_h = 0,
			-- (Kept laid out, not drawn: shown again, nothing is laid out anew.
			-- Its readers are held meanwhile, ds/hold: nothing in it changes.)
			invisible = function(): boolean
				return M.tab:get() ~= tab
			end,
			pointer_events = function(): any
				return if M.tab:get() == tab then "auto" else "none"
			end,
			-- (While it's still being built in the background, out of layout
			-- altogether: laid out once, when its last piece is in.)
			ui.column {
				w_full = true,
				h_full = true,
				min_h = 0,
				hidden = function(): boolean
					return M.tab:get() ~= tab and (stage.pending:get()[tab] or 0) > 0
				end,
				build(),
			},
		}
	end, { absolute = true, left = 0, right = 0, top = 0, bottom = 0 })
end

M.Page = ui.component(function(): Node
	fresh()
	local width = ui.use_state(0)
	-- The tab shown is built; the others once the page has settled.
	M.built:set({ [M.tab:peek()] = true })
	M.awake:set(M.tab:peek())
	ui.effect(function()
		local tab = M.tab:get()
		if M.awake:peek() ~= tab then
			-- (A frame's room first: the tab drawn as it was.)
			ui.after(16, function()
				if M.tab:peek() == tab then
					M.awake:set(tab)
				end
			end)
		end
	end)
	ui.effect(function()
		local tab = M.tab:get()
		if not M.built:peek()[tab] then
			M.built:set_key({ tab }, true)
		end
	end)
	local alive = true
	ui.on_cleanup(function()
		alive = false
	end)
	ui.after(M.PREBUILD_AFTER, function()
		if alive then
			M.built:set({ overview = true, board = true, chat = true })
		end
	end)
	-- A dump asked to be shown from another page (a board card's source):
	-- on Overview, where Capture's history is.
	ui.effect(function()
		local id = orchestration.reveal_dump:get()
		if id then
			ui.after(0, function()
				orchestration.reveal_dump:set(nil)
				M.show_tab("overview")
				ui.after(0, function()
					M.show_dump(id)
				end)
			end)
		end
	end)
	ui.on_cleanup(function()
		decide.hide()
		if M.floating:peek() then
			M.floating:set(false)
		end
		if M.float_open:peek() then
			M.float_open:set(false)
		end
	end)
	-- Needs you floating: off the page's foot, and over the floating
	-- composer where the page has no room for both side by side.
	local function needs_bottom(): number
		local both = FLOAT_W + 2 * (NEEDS_W + FLOAT_SIDE)
		if width:get() < both then
			return M.float_h:get() + FLOAT_FOOT + FLOAT_CLEAR
		end
		return FLOAT_FOOT
	end
	-- (The page's width, for Chat to tell whether Needs you reaches its
	-- transcript.)
	ui.effect(function()
		M.page_w:set(width:get())
	end)
	-- The floating composer: on Board and Chat, and on Overview with
	-- Capture scrolled away.
	local float_shown = ui.computed(function(): boolean
		return M.tab:get() ~= "overview" or M.floating:get()
	end)
	return ui.column {
		id = "control-room",
		w_full = true,
		h_full = true,
		min_h = 0,
		relative = true,
		-- The head (the tabs in it), over every tab.
		ui.column {
			w_full = true,
			items = "center",
			px = "$space.6",
			pt = "$space.4",
			pb = "$space.4",
			-- (The page's width, as every tab lays it out: one column or two,
			-- the floating composer's room.)
			on_resize = function(size: { w: number, h: number })
				if math.abs(size.w - width:peek()) >= 1 then
					width:set(size.w)
				end
			end,
			ui.column { w_full = true, max_w = MAX_W, head() },
		},
		-- The tabs: each built the first time it's shown (or in the
		-- background, once the page has settled) and kept, hidden while
		-- another shows: going back to one is showing it.
		ui.column {
			w_full = true,
			grow = 1,
			min_h = 0,
			relative = true,
			kept("overview", function(): Node
				return Overview { width = width }
			end),
			kept("board", function(): Node
				return BoardTab {}
			end),
			kept("chat", function(): Node
				return ChatTab {}
			end),
		},
		-- Capture's composer, floating: kept built, shown where it floats
		-- (faded in; gone at once).
		ui.column {
			id = "control-float-layer",
			absolute = true,
			left = 0,
			right = 0,
			bottom = 0,
			pointer_events = "none",
			invisible = function(): boolean
				return not float_shown:get()
			end,
			opacity = motion.tween(ui.computed(function(): number
				return if float_shown:get() then 1 else 0
			end), { duration = 160, ease = "out(3)" }),
			FloatingCapture {
				width = function(): number
					return width:get()
				end,
				shown = function(): boolean
					return float_shown:get()
				end,
			},
		},
		-- Needs you, floating, on Board and Chat while something waits.
		ui.presence(function(): string?
			if M.tab:get() == "overview" or tally.needs(false) == 0 then
				return nil
			end
			return "needs"
		end, function(): Node
			return NeedsFloat { bottom = needs_bottom }
		end, {
			absolute = true,
			right = FLOAT_SIDE,
			bottom = needs_bottom,
			pointer_events = "none",
			enter = motion.transition({ opacity = { 0, 1 }, y = { 8, 0 }, duration = 180, ease = "out(3)" }),
			exit = motion.transition({ opacity = { 1, 0 }, duration = 120 }),
		}),
		decision(),
		qa_dialog(),
	}
end)

return M

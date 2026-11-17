-- Progress, the app's side of ds/orch_progress: the engine's rows
-- (account/orchestration's `progress`) joined with the chats they're of;
-- a chat working (or waiting) that has none (its host writes none yet) is
-- drawn from what its session row says: its state, how long it has been at
-- it (since this app saw it start, `orchestration.timeline`), what it's
-- doing, where; no steps, no note;
-- the threads pinned to the Control room's Progress widget (this device's),
-- and "What's up?" (a fresh note asked of the host, never of the agent),
-- held off for 15 seconds after it's used.
--
--   progress.Widget()            the Control room's widget: every thread
--                                working now (the latest started first) and
--                                those pinned (first)
--   progress.strip(chat, opts?)  one line, where the chat has a row (crew
--                                rows and board cards: `{ lean = true }`)
--   progress.chip(chat)          tiny, where the chat has a row
--   progress.Header(id)          a working session's line, in its head
--   progress.pin(chat, on), progress.pinned(chat), progress.check(chat)
--   progress.figures()           the widget's live figures
--   progress.counts()            how many work now, how many stalled

local ds = require("@ds")
local Unit = require("@ds/orch_progress")
local Block = require("@ds/orch_block")
local data = require("@ds/orch_data")
local Ref = require("@ds/orch_ref")
local locale = require("@ds/locale")
local orchestration = require("@account/orchestration")
local hold = require("@ds/hold")
local sessions = require("@account/sessions")
local rows = require("@account/rows")
local route = require("@app/screens/shell/route")

local M = {}

-- How long "What's up?" rests after it's used (seconds).
M.COOLDOWN = 15
-- The widget's units before the rest fold away.
M.FIRST = 4

-- The threads pinned to the widget, in the order they were pinned (this
-- device's).
M.pins = ui.persist("progress_pins", {} :: { string })
-- Those asked about this moment (their reply not back yet), and those
-- whose "What's up?" rests.
local asking = ui.state("progress_asking", {} :: { [string]: boolean })
local resting = ui.state("progress_resting", {} :: { [string]: boolean })
-- A test's clock (Unix seconds): set before the units are built, they
-- read it in place of their own.
local clock = ui.state("progress_now", nil :: number?)
-- The widget showing all its units, not the first few.
local all = ui.state("progress_all", false)

-- (What this module derives is worked out as it's read, never kept in a
-- ui.computed made here: this module is first required by whichever screen
-- shows Progress first (screens/shell/boundary builds screens), and a
-- computed made at the top of a module required inside a build belongs to
-- that build. Gone with it (the Control room left), it would keep its last
-- value for every screen after: Progress showing nothing working.)
-- Each is worked out again only when what it's made from changes: its
-- `sources` are read on every `get` (so a reader still follows them), and
-- the last answer is given back while they're the same. A large shared
-- value keeps its table while it stands; a small one is read anew each
-- time, and compared through (it's small).
local function same(a: any, b: any, left: { n: number }): boolean
	if rawequal(a, b) then
		return true
	end
	if type(a) ~= "table" or type(b) ~= "table" then
		return false
	end
	for k, v in a do
		left.n -= 1
		if left.n < 0 or not same(v, b[k], left) then
			return false
		end
	end
	for k in b do
		left.n -= 1
		if left.n < 0 or a[k] == nil then
			return false
		end
	end
	return true
end
local function derived<T>(sources: () -> ...any, f: (...any) -> T): { get: (any) -> T }
	local last: any = nil
	local value: any = nil
	return {
		get = function(_self: any): T
			local now = table.pack(sources())
			if last and last.n == now.n then
				local hit = true
				for i = 1, now.n do
					if not same(now[i], last[i], { n = 256 }) then
						hit = false
						break
					end
				end
				if hit then
					return value
				end
			end
			value = f(table.unpack(now, 1, now.n))
			last = now
			return value
		end,
	}
end

-- A script-owned computed of `f`, handing its readers the very table it
-- gave before while what it says is the same: a push of sessions or the
-- timeline that changes no row wakes none of the rows' readers.
local function stable<T>(f: () -> T): { get: (any) -> T, peek: (any) -> T }
	local prev: any = nil
	return ui.computed(function(): T
		local v = f()
		if prev ~= nil and same(v, prev, { n = 8192 }) then
			return prev
		end
		prev = v
		return v
	end) :: any
end
-- The minute, as state: what a row counts as fresh moves on with it.
local minute = ui.state("progress_minute", os.time() // 60)
ui.every(60000, function()
	minute:set(os.time() // 60)
end)

local chats_now = derived(function(): any
	return sessions.now:get()
end, function(s: any): { [string]: any }
	return data.chats(s)
end)
local leads_now = derived(function(): any
	return orchestration.now:get()
end, function(now: any): { [string]: any }
	return data.orchestrators(now)
end)
-- Chats a check found nothing to say about yet.
local quiet = ui.state("progress_quiet", {} :: { [string]: boolean })
-- What each chat has done lately (src/orchestration_timeline.rs).
local timeline = ui.shared("orchestration.timeline", {} :: { [string]: { any } })

-- A session's status as a progress row's state.
local STATE: { [string]: string } = { working = "working", waiting = "waiting", input = "awaiting_input", approval = "awaiting_input" }

-- A chat made this recently is taken to be at the task it was made for:
-- its stretch began when it was made (seconds).
local FRESH = 3 * 3600

-- When its current stretch began, as best the session says: when it was
-- made, if that was lately (a crew thread, a task's chat); else the first
-- of its latest unbroken run of busy entries as this app saw them; else
-- when its activity began; else its last change.
local function stretch_start(c: any, lines: { [string]: { any } }): number?
	if type(c.created) == "number" and os.time() - c.created <= FRESH then
		return c.created
	end
	local entries = lines[c.id]
	local start: number? = nil
	if entries then
		for i = #entries, 1, -1 do
			local e = entries[i]
			if STATE[e.status or ""] then
				start = e.at
			else
				break
			end
		end
	end
	local a = c.activity
	local since = if type(a) == "table" and type(a.since) == "number" then a.since else nil
	if start and since then
		return math.min(start, since)
	end
	return start or since or c.updated
end

-- Every chat at work now that isn't archived: a row drawn from its session.
-- (Worked out again each minute too: a chat made lately stops counting as
-- made for its task as it ages.)
local stood_in = derived(function(): ...any
	return sessions.now:get(), timeline:get(), minute:get()
end, function(now: any, lines: any): { [string]: any }
	local out: { [string]: any } = {}
	local function take(c: any)
		local state = STATE[c.status or ""]
		if state then
			out[c.id] = {
				chat_id = c.id,
				device_id = c.device,
				state = state,
				started = stretch_start(c, lines or {}),
				stalled = false,
				refreshing = false,
				updated = c.updated,
				-- (Not the engine's: drawn from the session.)
				session_only = true,
			}
		end
	end
	if now then
		for _, c in now.chats or {} do
			take(c)
		end
		for _, kids in now.children or {} do
			for _, c in kids do
				take(c)
			end
		end
	end
	return out
end)

-- Every row: the engine's (its proxies too), and one for each chat at work
-- it has none for; a thread that ended its turn asking the person (an
-- open ask raised from its turn's end) and is at rest since (idle or done;
-- not an orchestrator's own chat) waits on them, from when it asked.
local rows_now = derived(function(): ...any
	return orchestration.progress:get(), stood_in:get(), orchestration.now:get(), chats_now:get(), leads_now:get()
end, function(progress: any, stood: any, now: any, chats: any, leads: any): { [string]: any }
	local out = table.clone(progress or {})
	for id, r in stood do
		if out[id] == nil then
			out[id] = r
		end
	end
	for id, a in data.open_asks(now) do
		local r = out[id]
		local state = r and r.state
		local c = chats[id]
		if data.turn_end(a) and not leads[id] and (r or c) and (state == nil or state == "done") then
			local w = if r then table.clone(r) else { chat_id = id, device_id = c and c.device, stalled = false, refreshing = false }
			w.state = "awaiting_input"
			w.asking = true
			w.started = a.created or w.started
			w.updated = a.updated or a.created or w.updated
			out[id] = w
		end
	end
	return out
end)

M.rows = stable(function(): { [string]: any }
	return rows_now:get()
end)

local function with(map: State<{ [string]: boolean }>, id: string, on: boolean)
	local copy = table.clone(map:peek())
	copy[id] = if on then true else nil
	map:set(copy)
end

function M.pinned(id: string): boolean
	return table.find(M.pins:get(), id) ~= nil
end

function M.pin(id: string, on: boolean)
	local list = table.clone(M.pins:peek())
	local at = table.find(list, id)
	if on and not at then
		table.insert(list, id)
	elseif not on and at then
		table.remove(list, at)
	end
	M.pins:set(list)
end

-- A fresh note asked for: once, then not again for `COOLDOWN` seconds.
function M.check(id: string)
	if resting:peek()[id] then
		return
	end
	with(resting, id, true)
	with(asking, id, true)
	ui.after(M.COOLDOWN * 1000, function()
		with(resting, id, false)
	end)
	-- (Nothing back, or nothing to say, or a host that can't be asked yet:
	-- "No details yet", quietly.)
	orchestration.check(id):after(function(row: any)
		with(asking, id, false)
		with(quiet, id, type(row) ~= "table" or ((row.note == nil or row.note == "") and not row.refreshing))
	end, function(_e: any)
		with(asking, id, false)
		with(quiet, id, true)
	end)
end

-- A chat opened: an orchestrator's on its own page, any other as a session.
function M.open(id: string)
	for _, o in orchestration.now:peek().orchestrators or {} do
		if o.id == id then
			route.orchestrator(id)
			return
		end
	end
	route.open_session(id)
end

-- Each row by itself: a reader of one chat's row runs again when that row
-- says something new, not whenever any row does. One effect, the script's,
-- walks the rows as they change and bumps the version of each that did
-- (compared by what they say: a row rebuilt the same wakes nobody).
local row_held: { [string]: any } = {}
local row_versions: { [string]: State<number> } = {}
local function row_version(id: string): State<number>
	local v = row_versions[id]
	if not v then
		v = ui.state(`progress_row:{id}`, 0)
		row_versions[id] = v
	end
	return v
end
local function take_rows(all: { [string]: any })
	local bump: { string } = {}
	for id, r in all do
		local was = row_held[id]
		if was ~= r then
			row_held[id] = r
			if was == nil or not same(r, was, { n = 512 }) then
				table.insert(bump, id)
			end
		end
	end
	for id in row_held do
		if all[id] == nil then
			row_held[id] = nil
			table.insert(bump, id)
		end
	end
	if #bump > 0 then
		ui.batch(function()
			for _, id in bump do
				local v = row_versions[id]
				if v then
					v:set(v:peek() + 1)
				end
			end
		end)
	end
end
take_rows(M.rows:peek())
ui.effect(function()
	take_rows(M.rows:get())
end)
local function row_of(id: string): any
	row_version(id):get()
	return row_held[id]
end
-- A chat's progress row (nil while it has none): what a board card's
-- clocks read.
M.row = row_of

-- Whose work it is and where it runs: its orchestrator (or project) and
-- its machine.
function M.context(id: string): string
	-- (Its chat, its crew row and its orchestrator, each by itself.)
	local c = rows.chat(id)
	local parts = {}
	local m = rows.member(id)
	local orchestrator = m and rows.orchestrator(m.orchestrator_id)
	if orchestrator then
		table.insert(parts, orchestrator.name)
	elseif c and c.project then
		for _, p in sessions.projects:get() or {} do
			if p.id == c.project then
				table.insert(parts, p.name)
				break
			end
		end
	end
	local device = (c and c.device) or (row_of(id) or {}).device_id
	for _, d in sessions.devices:get() or {} do
		if d.id == device then
			table.insert(parts, d.name)
			break
		end
	end
	return table.concat(parts, " · ")
end

-- (`live`: whether what shows it is shown; while not, what it reads is
-- held at its last value, ds/hold.)
local function unit(id: string, variant: Unit.Variant, lean: boolean?, live: (() -> boolean)?): Node
	local fixed = clock:peek()
	local function held<T>(f: () -> T): () -> T
		return if live then hold.fn(live, f) else f
	end
	return Unit.Progress {
		id = `progress-{variant}-{id}`,
		variant = variant,
		lean = lean,
		row = held(function(): any
			return row_of(id)
		end),
		placeholder = held(function(): string?
			return if quiet:get()[id] then locale.s("control.progress.no_details") else nil
		end),
		chat = held(function(): any
			return rows.chat(id)
		end),
		context = held(function(): string
			return M.context(id)
		end),
		now = if fixed
			then function(): number
				return clock:get() or os.time()
			end
			else nil,
		on_check = function()
			M.check(id)
		end,
		checking = function(): boolean
			return asking:get()[id] == true
		end,
		cooling = function(): boolean
			return resting:get()[id] == true
		end,
		on_open = function()
			M.open(id)
		end,
		pinned = function(): boolean
			return M.pinned(id)
		end,
		on_pin = function(on: boolean)
			M.pin(id, on)
		end,
	}
end
M.unit = unit

-- One line, while the chat has a row.
function M.strip(id: string, opts: { lean: boolean? }?): Node
	return ui.show(function(): boolean
		return row_of(id) ~= nil
	end, unit(id, "strip", opts and opts.lean))
end

function M.chip(id: string): Node
	return ui.show(function(): boolean
		return row_of(id) ~= nil
	end, unit(id, "chip"))
end

-- The widget's threads: those pinned first (in their order), then every
-- one working now, the latest started first.
local listed_now = derived(function(): ...any
	return M.rows:get(), M.pins:get()
end, function(rows: any, pins: { string }): { { id: string } }
	local out, seen = {}, {}
	for _, id in pins do
		if rows[id] and not seen[id] then
			seen[id] = true
			table.insert(out, { id = id })
		end
	end
	local working = {}
	for id, r in rows do
		if Unit.ACTIVE[r.state or ""] and not seen[id] then
			table.insert(working, r)
		end
	end
	table.sort(working, function(a: any, b: any): boolean
		local sa, sb = a.started or a.updated or 0, b.started or b.updated or 0
		if sa ~= sb then
			return sa > sb
		end
		return a.chat_id < b.chat_id
	end)
	for _, r in working do
		table.insert(out, { id = r.chat_id })
	end
	return out
end)

M.listed = stable(function(): { { id: string } }
	return listed_now:get()
end)

-- How many work now, and how many of those are stalled: the widget's
-- figures, and the Control room's head (`progress.counts`), so the two
-- never disagree.
local function counts(): (number, number)
	local working, stalled = 0, 0
	for _, r in M.rows:get() do
		if r.state == "working" or r.state == "waiting" then
			working += 1
		end
		if Unit.stalled(r) then
			stalled += 1
		end
	end
	return working, stalled
end

M.counts = counts

function M.figures(live: (() -> boolean)?): Node
	local function figure_if(text: () -> string, tone: string?): Node
		if live then
			text = hold.fn(live, text)
		end
		return ui.show(function(): boolean
			return text() ~= ""
		end, Block.figure(text, tone))
	end
	return ui.row {
		gap = "$space.3",
		items = "center",
		figure_if(function(): string
			local w = counts()
			return if w > 0 then locale.s("control.map.working", { n = w }) else ""
		end),
		figure_if(function(): string
			local _, s = counts()
			return if s > 0 then locale.s("control.figures.stalled", { n = s }) else ""
		end, "limited"),
	}
end

-- (`live`, as `unit`'s.)
function M.Widget(live: (() -> boolean)?): Node
	local listed = if live
		then hold.fn(live, function(): { { id: string } }
			return M.listed:get()
		end, "progress")
		else function(): { { id: string } }
			return M.listed:get()
		end
	local shown = ui.computed(function(): { { id: string } }
		local list = listed()
		if all:get() or #list <= M.FIRST + 1 then
			return list
		end
		return table.move(list, 1, M.FIRST, 1, {})
	end)
	local function more(): number
		return #listed() - #shown:get()
	end
	return ui.column {
		id = "control-progress",
		w_full = true,
		gap = "$space.3",
		ui.show(function(): boolean
			return #listed() == 0
		end, ui.text {
			locale.t("control.progress.empty"),
			class = "control-progress-empty",
			text_size = "$font.size.sm",
			color = "$text.tertiary",
		}),
		ui.each(shown, function(e: { id: string }): Node
			-- Dragged: a reference to its chat (ds/orch_ref), for Capture.
			local drag = Ref.source(function(): any
				local r = row_of(e.id) or {}
				local c = chats_now:get()[e.id] or {}
				return Ref.make("chat", e.id, c.title, { note = r.note, state = r.state })
			end)
			return ui.column {
				class = "control-progress-row",
				on_drag = drag.on_drag,
				drag_preview = drag.drag_preview,
				drag_preview_anchor = drag.drag_preview_anchor,
				drag_preview_offset = drag.drag_preview_offset,
				w_full = true,
				gap = "$space.3",
				-- (A hairline over each but the first.)
				ui.show(function(): boolean
					local top = shown:get()[1]
					return top ~= nil and top.id ~= e.id
				end, ui.div { w_full = true, h = 1, bg = "$border.subtle" }),
				unit(e.id, "full", nil, live),
			}
		end, {
			key = function(e: { id: string }): string
				return e.id
			end,
			w_full = true,
			gap = "$space.3",
		}),
		-- The rest, folded: how many, or fold them again.
		ui.show(function(): boolean
			return all:get() or more() > 0
		end, ds.Button {
			id = "control-progress-more",
			class = "control-progress-more",
			tone = "ghost",
			size = "xs",
			on_click = function()
				all:set(not all:peek())
			end,
			ui.text {
				function(): string
					return if all:get() and more() == 0 then locale.s("ds.orch.less") else locale.s("ds.orch.more", { n = more() })
				end,
			},
		}),
	}
end

-- A working session's line, in its head (the shell's `#session-header`):
-- shown while its work is under way (or stalled), the one line centred.
function M.Header(id: () -> string?): Node
	return ui.presence(function(): string
		local s = id() or ""
		local r = if s ~= "" then row_of(s) else nil
		if not r or not (Unit.ACTIVE[r.state or ""] or Unit.stalled(r)) then
			return ""
		end
		return s
	end, function(s: string): Node
		if s == "" then
			return ui.div {}
		end
		return ui.row {
			id = "chat-progress",
			w_full = true,
			min_w = 0,
			justify = "center",
			ui.row {
				pointer_events = "auto",
				w_full = true,
				min_w = 0,
				max_w = 560,
				unit(s, "strip"),
			},
		}
	end, { grow = 1, min_w = 0 })
end

return M

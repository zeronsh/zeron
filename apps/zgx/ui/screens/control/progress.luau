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
local locale = require("@ds/locale")
local orchestration = require("@account/orchestration")
local sessions = require("@account/sessions")
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
local function derived<T>(f: () -> T): { get: (any) -> T }
	return {
		get = function(_self: any): T
			return f()
		end,
	}
end

local chats_now = derived(function(): { [string]: any }
	return data.chats(sessions.now:get())
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
local function stretch_start(c: any): number?
	if type(c.created) == "number" and os.time() - c.created <= FRESH then
		return c.created
	end
	local entries = timeline:get()[c.id]
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
local stood_in = derived(function(): { [string]: any }
	local now = sessions.now:get()
	local out: { [string]: any } = {}
	local function take(c: any)
		local state = STATE[c.status or ""]
		if state then
			out[c.id] = {
				chat_id = c.id,
				device_id = c.device,
				state = state,
				started = stretch_start(c),
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
M.rows = derived(function(): { [string]: any }
	local out = table.clone(orchestration.progress:get())
	for id, r in stood_in:get() do
		if out[id] == nil then
			out[id] = r
		end
	end
	local now = orchestration.now:get()
	local leads = data.orchestrators(now)
	for id, a in data.open_asks(now) do
		local r = out[id]
		local state = r and r.state
		local c = chats_now:get()[id]
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

local function row_of(id: string): any
	return M.rows:get()[id]
end
-- A chat's progress row (nil while it has none): what a board card's
-- clocks read.
M.row = row_of

-- Whose work it is and where it runs: its orchestrator (or project) and
-- its machine.
function M.context(id: string): string
	local now = orchestration.now:get()
	local c = chats_now:get()[id]
	local parts = {}
	local m = data.crew_by_chat(now)[id]
	local orchestrator = m and data.orchestrators(now)[m.orchestrator_id]
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

local function unit(id: string, variant: Unit.Variant, lean: boolean?): Node
	local fixed = clock:peek()
	return Unit.Progress {
		id = `progress-{variant}-{id}`,
		variant = variant,
		lean = lean,
		row = function(): any
			return row_of(id)
		end,
		placeholder = function(): string?
			return if quiet:get()[id] then locale.s("control.progress.no_details") else nil
		end,
		chat = function(): any
			return chats_now:get()[id]
		end,
		context = function(): string
			return M.context(id)
		end,
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
M.listed = derived(function(): { { id: string } }
	local rows = M.rows:get()
	local out, seen = {}, {}
	for _, id in M.pins:get() do
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

function M.figures(): Node
	local function figure_if(text: () -> string, tone: string?): Node
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

function M.Widget(): Node
	local shown = ui.computed(function(): { { id: string } }
		local list = M.listed:get()
		if all:get() or #list <= M.FIRST + 1 then
			return list
		end
		return table.move(list, 1, M.FIRST, 1, {})
	end)
	local function more(): number
		return #M.listed:get() - #shown:get()
	end
	return ui.column {
		id = "control-progress",
		w_full = true,
		gap = "$space.3",
		ui.show(function(): boolean
			return #M.listed:get() == 0
		end, ui.text {
			locale.t("control.progress.empty"),
			class = "control-progress-empty",
			text_size = "$font.size.sm",
			color = "$text.tertiary",
		}),
		ui.each(shown, function(e: { id: string }): Node
			return ui.column {
				class = "control-progress-row",
				w_full = true,
				gap = "$space.3",
				-- (A hairline over each but the first.)
				ui.show(function(): boolean
					local top = shown:get()[1]
					return top ~= nil and top.id ~= e.id
				end, ui.div { w_full = true, h = 1, bg = "$border.subtle" }),
				unit(e.id, "full"),
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

-- Progress, the app's side of ds/orch_progress: the engine's rows
-- (account/orchestration's `progress`) joined with the chats they're of,
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

local chats_now = ui.computed(function(): { [string]: any }
	return data.chats(sessions.now:get())
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
	orchestration.check(id):after(function()
		with(asking, id, false)
	end, function(_e: any)
		with(asking, id, false)
		ds.toast.show { title = locale.s("control.progress.failed"), tone = "danger" }
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
	return orchestration.progress:get()[id]
end

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
M.listed = ui.computed(function(): { { id: string } }
	local rows = orchestration.progress:get()
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

-- How many work now, and how many of those are stalled.
local function counts(): (number, number)
	local working, stalled = 0, 0
	for _, r in orchestration.progress:get() do
		if r.state == "working" or r.state == "waiting" then
			working += 1
		end
		if Unit.stalled(r) then
			stalled += 1
		end
	end
	return working, stalled
end

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

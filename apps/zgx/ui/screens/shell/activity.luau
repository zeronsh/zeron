-- The Activity sidebar: one list of the chats that need you, the one that
-- came back last at the top. A chat working folds into Working until it
-- stops (then it comes back to the top); one put to sleep folds into Asleep
-- until its time, or until it asks for you (a question, an approval, a
-- failure); archived ones sit on the Archived shelf. The three shelves stand
-- at the list's end, folded until opened.
--
-- Turned on from the sidebar's "…" (or Settings → Activity), with its own
-- settings on that page: whether working chats fold away, and how long a
-- chat may sit idle before it's archived. Off, the sidebar is as it was.
--
-- Kept on this device: the view, its settings, and which chats are asleep
-- until when (the engine has no word for sleep, so another device shows
-- them awake).
--
--   activity.on                 the view (a persisted boolean)
--   activity.working_shelf      working chats fold into Working
--   activity.archive_after      days idle before a chat is archived (0: never)
--   activity.shelf(id, status)  "working", "asleep" or nil (the list), tracked
--   activity.rank(chat)         its place: higher, nearer the top, tracked
--   activity.sleep(id, wake_at)   asleep until `wake_at` (Unix seconds)
--   activity.wake(id)           awake now, at the list's top
--   activity.presets()          Sleep's choices: { id, label, detail, wake_at }
--   activity.wakes_in(id)       how soon it wakes ("2h"), for its row

local sessions = require("@account/sessions")
local locale = require("@ds/locale")
local route = require("@app/screens/shell/route")

local M = {}

M.on = ui.persist("shell_sidebar_activity", false)
M.working_shelf = ui.persist("activity_working_shelf", true)
M.archive_after = ui.persist("activity_archive_after", 0)

-- A chat asleep: until when, when it was put to sleep, and what it was
-- asking then (an ask already there when it slept doesn't wake it; a new
-- one does).
export type Sleep = { wake_at: number, at: number, held: string? }
M.sleeping = ui.persist("activity_sleeping", {} :: { [string]: Sleep })
-- When a chat last came back on its own (woke, or stopped working): it
-- stands by that, or by its own last move, whichever is later.
local returned = ui.persist("activity_returned", {} :: { [string]: number })

-- What an agent asks of you, by chat (the chat page's): which of "input" a
-- chat waiting on you is.
local asks = ui.shared("chat.asks", {} :: { [string]: string })
local ASKED: { [string]: string } = { approval = "approval", question = "input" }
local function status_of(c: sessions.Chat): string?
	return ASKED[asks:get()[c.id] or ""] or c.status
end

-- The states that need you: they wake a sleeping chat early, and a chat in
-- one is never archived on its own.
local NEEDS_YOU: { [string]: boolean } = { input = true, approval = true, failed = true, limited = true }
local WORKING: { [string]: boolean } = { working = true, connecting = true }
-- A chat that stops working stays in Working this long: an agent often ends
-- a turn a moment before its next one starts, and the row shouldn't hop.
local GRACE_MS = 2000

-- The clock the shelves are sorted by: moved when a sleeper's time comes,
-- a grace runs out, and once a minute (auto-archive's beat), so what reads
-- it follows the time without a clock of its own.
local tick = ui.state("activity_tick", 0)
local function bump()
	tick:set(tick:peek() + 1)
end
local function now(): number
	tick:get()
	return os.time()
end

-- Chats in their grace after working, by id: when it runs out (ms, ui.now).
local graced: { [string]: number } = {}
local was_status: { [string]: string } = {}
-- The archived ones last seen: one back from there (unarchived) counts as
-- just come back, so it isn't archived again for being old.
local was_archived: { [string]: boolean }? = nil

function M.shelf(id: string, status: string?): string?
	if not M.on:get() then
		return nil
	end
	local s = M.sleeping:get()[id]
	if s and s.wake_at > now() and not (status and NEEDS_YOU[status] and status ~= s.held) then
		return "asleep"
	end
	if M.working_shelf:get() and ((status ~= nil and WORKING[status]) or graced[id] ~= nil) then
		return "working"
	end
	return nil
end

-- Higher stands nearer the top: Activity by when it last came back (its
-- own last move, or waking, or stopping work); Asleep by how soon it wakes.
function M.rank(c: sessions.Chat, shelf: string?): number
	if shelf == "asleep" then
		local s = M.sleeping:get()[c.id]
		return if s then -s.wake_at else 0
	end
	local back = returned:get()[c.id] or 0
	return math.max(c.updated or 0, back)
end

local function set_returned(id: string, at: number)
	local r = table.clone(returned:peek())
	r[id] = at
	returned:set(r)
end

function M.sleep(id: string, wake_at: number)
	local c: sessions.Chat? = nil
	for _, chat in sessions.now:peek().chats do
		if chat.id == id then
			c = chat
		end
	end
	local held: string? = nil
	if c then
		local st = ASKED[asks:peek()[id] or ""] or c.status
		if st and NEEDS_YOU[st] then
			held = st
		end
	end
	local s = table.clone(M.sleeping:peek())
	s[id] = { wake_at = wake_at, at = os.time(), held = held }
	M.sleeping:set(s)
end

function M.wake(id: string)
	local s = M.sleeping:peek()
	if s[id] == nil then
		return
	end
	local next = table.clone(s)
	next[id] = nil
	M.sleeping:set(next)
	set_returned(id, os.time())
end

function M.asleep(id: string): boolean
	local s = M.sleeping:get()[id]
	return s ~= nil and s.wake_at > now()
end

-- How soon it wakes, as its row's corner says it: "45m", "3h", "2d".
function M.wakes_in(id: string): string?
	local s = M.sleeping:get()[id]
	if not s then
		return nil
	end
	local left = math.max(0, s.wake_at - now())
	if left < 3600 then
		return locale.s("time.short.minutes", { n = math.max(1, math.ceil(left / 60)) })
	elseif left < 86400 then
		return locale.s("time.short.hours", { n = math.ceil(left / 3600) })
	end
	return locale.s("time.short.days", { n = math.ceil(left / 86400) })
end

-- Sleep's choices, soonest first, one a minute: in an hour, in three, this
-- evening (6 PM, while it's still ahead), tomorrow at 9, next Monday at 9.
-- Each says when it wakes: its time, and its day where its name doesn't.
export type Preset = { id: string, label: string, detail: string, wake_at: number }
function M.presets(): { Preset }
	local t = os.time()
	local today = os.date("*t", t) :: any
	local function at(days: number, hour: number): number
		return os.time({ year = today.year, month = today.month, day = today.day + days, hour = hour, min = 0, sec = 0 }) :: number
	end
	local out: { Preset } = {}
	local function add(id: string, key: string, wake_at: number, names_day: boolean)
		if wake_at <= t then
			return
		end
		local same_day = os.date("%Y-%m-%d", wake_at) == os.date("%Y-%m-%d", t)
		local detail = if names_day or same_day then locale.clock(wake_at) else `{locale.weekday(wake_at)} {locale.clock(wake_at)}`
		table.insert(out, { id = id, label = locale.s(key), detail = detail, wake_at = wake_at })
	end
	add("hour", "shell.activity.sleep.in_hour", t + 3600, false)
	add("hours", "shell.activity.sleep.in_hours", t + 3 * 3600, false)
	add("evening", "shell.activity.sleep.evening", at(0, 18), true)
	add("tomorrow", "shell.activity.sleep.tomorrow", at(1, 9), true)
	-- (Monday: a week on from a Monday; the next one from any other day.)
	local to_monday = (9 - today.wday) % 7
	add("week", "shell.activity.sleep.next_week", at(if to_monday == 0 then 7 else to_monday, 9), false)
	table.sort(out, function(a: Preset, b: Preset): boolean
		return a.wake_at < b.wake_at
	end)
	local seen: { [number]: boolean } = {}
	local kept: { Preset } = {}
	for _, p in out do
		local minute = math.floor(p.wake_at / 60)
		if not seen[minute] then
			seen[minute] = true
			table.insert(kept, p)
		end
	end
	return kept
end

-- The days Settings offers before an idle chat is archived (0: never).
M.ARCHIVE_AFTER = { 0, 1, 3, 7, 14, 30 }

-- Following the chats: one that stops working stays in Working for its
-- grace, then comes back to the top (at once if it needs you); a sleeper
-- whose time came, or who asks for you with something new, wakes (back at
-- the top); one gone (archived, deleted) forgets its sleep.
local timer_gen = 0
ui.effect(function()
	local on = M.on:get()
	local now_state = sessions.now:get()
	asks:get()
	tick:get()
	if not on or now_state.state ~= "ready" then
		return
	end
	local t = os.time()
	local ms = ui.now()
	local here: { [string]: sessions.Chat } = {}
	local woke: { [string]: number } = {}
	-- (The grace is no state: a change to it moves the clock, so the list
	-- reads it again.)
	local grace_moved = false
	for _, c in now_state.chats do
		here[c.id] = c
		local st = status_of(c) or "idle"
		local was = was_status[c.id]
		was_status[c.id] = st
		if was and WORKING[was] and not WORKING[st] then
			if NEEDS_YOU[st] then
				grace_moved = grace_moved or graced[c.id] ~= nil
				graced[c.id] = nil
				woke[c.id] = t
			else
				graced[c.id] = ms + GRACE_MS
				grace_moved = true
			end
		elseif WORKING[st] and graced[c.id] ~= nil then
			graced[c.id] = nil
			grace_moved = true
		end
		if was_archived and was_archived[c.id] then
			woke[c.id] = t
		end
	end
	was_archived = {}
	for _, c in now_state.archived or {} do
		(was_archived :: { [string]: boolean })[c.id] = true
	end
	for id, wake_at in graced do
		if not here[id] then
			graced[id] = nil
			grace_moved = true
		elseif wake_at <= ms then
			graced[id] = nil
			grace_moved = true
			woke[id] = t
		end
	end
	if grace_moved then
		ui.after(0, bump)
	end
	-- (Followed: a chat put to sleep sets its own wake. What this writes
	-- back runs it once more, finding nothing to change.)
	local sleeping = M.sleeping:get()
	local next_sleeping: { [string]: Sleep }? = nil
	for id, s in sleeping do
		local c = here[id]
		local st = if c then status_of(c) else nil
		local gone = c == nil
		local time_came = s.wake_at <= t
		local asked = st ~= nil and NEEDS_YOU[st] and st ~= s.held
		-- (The ask it slept on has moved on: the next one wakes it.)
		local let_go = s.held ~= nil and st ~= s.held
		if gone or time_came or asked or let_go then
			local changed: { [string]: Sleep } = next_sleeping or table.clone(sleeping)
			next_sleeping = changed
			if gone or time_came or asked then
				changed[id] = nil
				if not gone then
					woke[id] = if time_came then s.wake_at else t
				end
			else
				changed[id] = { wake_at = s.wake_at, at = s.at, held = nil }
			end
		end
	end
	if next(woke) ~= nil then
		local r = table.clone(returned:peek())
		for id, at in woke do
			r[id] = at
		end
		-- (Only those still listed: the rest would pile up.)
		for id in r do
			if not here[id] then
				r[id] = nil
			end
		end
		returned:set(r)
	end
	if next_sleeping then
		M.sleeping:set(next_sleeping)
	end
	-- The next time something moves on its own: a grace running out, a
	-- sleeper's time coming.
	local soonest: number? = nil
	for _, wake_at in graced do
		local d = wake_at - ms
		soonest = if soonest then math.min(soonest, d) else d
	end
	for _, s in next_sleeping or sleeping do
		local d = (s.wake_at - t) * 1000
		soonest = if soonest then math.min(soonest, d) else d
	end
	timer_gen += 1
	local gen = timer_gen
	if soonest then
		ui.after(math.clamp(soonest + 50, 50, 60 * 1000), function()
			if gen == timer_gen then
				bump()
			end
		end)
	end
end)

-- Auto-archive: once a minute, while the view is on and a number of days is
-- set, each chat idle longer than that is archived (on the Archived shelf,
-- one click from back). Never one that's pinned, working, asleep, asking
-- for you or failed, or the one open now; nothing until the places are
-- known (or a pinned chat could look unpinned).
local function sweep()
	local days = M.archive_after:peek()
	local now_state = sessions.now:peek()
	if not M.on:peek() or type(days) ~= "number" or days <= 0 then
		return
	end
	if now_state.state ~= "ready" or not now_state.places_ready then
		return
	end
	local pinned: { [string]: boolean } = {}
	for _, id in now_state.places.pinned do
		pinned[id] = true
	end
	local open = route.chat:peek().session
	local cutoff = os.time() - days * 86400
	local back = returned:peek()
	local sleeping = M.sleeping:peek()
	for _, c in now_state.chats do
		local st = ASKED[asks:peek()[c.id] or ""] or c.status
		local idle_since = math.max(c.updated or 0, back[c.id] or 0)
		if
			not pinned[c.id]
			and c.id ~= open
			and sleeping[c.id] == nil
			and not (st and (WORKING[st] or NEEDS_YOU[st]))
			and idle_since > 0
			and idle_since < cutoff
		then
			sessions.archive(c.id)
		end
	end
end
local beat_gen = ui.state("activity_beat_gen", 0)
local function beat(gen: number)
	ui.after(60 * 1000, function()
		if beat_gen:peek() ~= gen then
			return
		end
		sweep()
		bump()
		beat(gen)
	end)
end
-- (A reload starts the beat again; the old one, seeing a new number, stops.)
beat_gen:set(beat_gen:peek() + 1)
beat(beat_gen:peek())
-- (Turned on, or the days changed: a sweep now, not a minute on.)
ui.effect(function()
	M.on:get()
	M.archive_after:get()
	sessions.now:get()
	ui.after(0, sweep)
end)

return M

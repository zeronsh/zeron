-- The Activity sidebar: one list of the chats that need you, the one that
-- came back last at the top. A chat under way folds into Working (running,
-- or waiting on background work that will wake it), one put to sleep into
-- Asleep until its time or until it asks for you, and archived ones sit on
-- the Archived shelf. The three shelves stand at the list's end, folded
-- until opened.
--
-- What a chat is doing and its sleep are the engine's (synced, the same on
-- every device: `sessions.now`'s chats carry `status`, `waiting`,
-- `sleep_until` and `woke`). The view and its settings are this device's.
--
-- Cheap by design: nothing here runs on a session's change but what the
-- sidebar reads (`shelf`, `rank`); its one timer ticks once a minute and
-- does work only while a chat is asleep (its countdown) or auto-archive is
-- on (its sweep).
--
--   activity.on                 the view (a persisted boolean)
--   activity.working_shelf      chats under way fold into Working
--   activity.archive_after      days idle before a chat is archived (0: never)
--   activity.shelf(c, status)   "working", "asleep" or nil (the list)
--   activity.rank(c, shelf)     its place: higher, nearer the top
--   activity.sleep(id, until)   asleep until `until` (Unix seconds)
--   activity.wake(id)           awake now
--   activity.presets()          Sleep's choices: { id, label, detail, until }
--   activity.wakes_in(until)    how soon that is, as a row's corner says it

local sessions = require("@account/sessions")
local locale = require("@ds/locale")
local toast = require("@ds/toast")
local route = require("@app/screens/shell/route")

local M = {}

M.on = ui.persist("shell_sidebar_activity", false)
M.working_shelf = ui.persist("activity_working_shelf", true)
M.archive_after = ui.persist("activity_archive_after", 0)

-- Under way: running, or waiting on work that will wake it.
local UNDER_WAY: { [string]: boolean } = { working = true, connecting = true, waiting = true }
-- What needs you: a chat in one is never archived on its own.
local NEEDS_YOU: { [string]: boolean } = { input = true, approval = true, failed = true, limited = true, woke = true }

-- The minute, while something reads by it (a sleeper's countdown, the
-- archive sweep); still otherwise.
local minute = ui.state("activity_minute", 0)

function M.shelf(c: sessions.Chat, status: string?): string?
	if not M.on:get() then
		return nil
	end
	if c.sleep_until then
		return "asleep"
	end
	if M.working_shelf:get() and status and UNDER_WAY[status] then
		return "working"
	end
	return nil
end

-- Higher stands nearer the top: the list by when it last came back (its own
-- last move, or waking); Asleep by how soon it wakes.
function M.rank(c: sessions.Chat, shelf: string?): number
	if shelf == "asleep" then
		return -(c.sleep_until or 0)
	end
	return math.max(c.updated or 0, c.woke or 0)
end

-- A sleep or a wake its host refused says so: never a click that does
-- nothing. (An engine from before Sleep doesn't know the request at all:
-- that one says what would let it.)
local function said(waking: boolean): (any) -> ()
	return function(err: any)
		local outdated = err == "outdated"
		toast.show {
			title = locale.s(if waking then "shell.activity.wake_failed" else "shell.activity.sleep.failed"),
			body = if outdated then locale.s("shell.activity.sleep.needs_update") else nil,
			tone = "danger",
		}
	end
end

function M.sleep(id: string, until_at: number)
	sessions.sleep(id, until_at):after(function() end, said(false))
end

function M.wake(id: string)
	sessions.sleep(id, nil):after(function() end, said(true))
end

-- How soon `until_at` is, as a row's corner says it: "45m", "3h", "2d".
function M.wakes_in(until_at: number): string
	minute:get()
	local left = math.max(0, until_at - os.time())
	if left < 3600 then
		return locale.s("time.short.minutes", { n = math.max(1, math.ceil(left / 60)) })
	elseif left < 86400 then
		return locale.s("time.short.hours", { n = math.ceil(left / 3600) })
	end
	return locale.s("time.short.days", { n = math.ceil(left / 86400) })
end

-- Local calendar fields as a moment. (Luau's `os.time` reads a table as UTC:
-- the zone's offset at that moment is taken back out, worked out again at
-- the moment found so a daylight-saving change between now and then lands
-- on the wall-clock time asked for.)
local function local_time(fields: { [string]: number }): number
	local as_utc = os.time(fields :: any)
	local function offset(t: number): number
		return os.time(os.date("*t", t) :: any) - t
	end
	local guess = as_utc - offset(as_utc)
	return as_utc - offset(guess)
end
M.local_time = local_time

-- Sleep's choices, soonest first, one a minute: in an hour, in three, this
-- evening (6 PM, while it's still ahead), tomorrow at 9, next Monday at 9.
-- Each says when it wakes: its time, and its day where its name doesn't.
export type Preset = { id: string, label: string, detail: string, until_at: number }
function M.presets(): { Preset }
	local t = os.time()
	local today = os.date("*t", t) :: any
	local function at(days: number, hour: number): number
		return local_time({ year = today.year, month = today.month, day = today.day + days, hour = hour, min = 0, sec = 0 })
	end
	local out: { Preset } = {}
	local function add(id: string, key: string, until_at: number, names_day: boolean)
		if until_at <= t then
			return
		end
		local same_day = os.date("%Y-%m-%d", until_at) == os.date("%Y-%m-%d", t)
		local detail = if names_day or same_day then locale.clock(until_at) else `{locale.weekday(until_at)} {locale.clock(until_at)}`
		table.insert(out, { id = id, label = locale.s(key), detail = detail, until_at = until_at })
	end
	add("hour", "shell.activity.sleep.in_hour", t + 3600, false)
	add("hours", "shell.activity.sleep.in_hours", t + 3 * 3600, false)
	add("evening", "shell.activity.sleep.evening", at(0, 18), true)
	add("tomorrow", "shell.activity.sleep.tomorrow", at(1, 9), true)
	-- (Monday: a week on from a Monday; the next one from any other day.)
	local to_monday = (9 - today.wday) % 7
	add("week", "shell.activity.sleep.next_week", at(if to_monday == 0 then 7 else to_monday, 9), false)
	table.sort(out, function(a: Preset, b: Preset): boolean
		return a.until_at < b.until_at
	end)
	local seen: { [number]: boolean } = {}
	local kept: { Preset } = {}
	for _, p in out do
		local m = math.floor(p.until_at / 60)
		if not seen[m] then
			seen[m] = true
			table.insert(kept, p)
		end
	end
	return kept
end

-- The days Settings offers before an idle chat is archived (0: never).
M.ARCHIVE_AFTER = { 0, 1, 3, 7, 14, 30 }

-- Chats brought back from Archived, by id: when (Unix seconds). One counts
-- as just moved, so being old doesn't send it straight back. Kept across
-- launches (a sweep after a reload must know too), pruned to the chats there.
local back = ui.persist("activity_unarchived", {} :: { [string]: number })
-- The archived ones as the last sweep saw them (none yet: the first sweep
-- after a launch only looks).
local seen_archived: { [string]: boolean }? = nil

-- Auto-archive: each chat idle longer than the days set goes to Archived
-- (one click from back). Never one that's pinned, open now, asleep, under
-- way or needing you; nothing until the places are known (or a pinned chat
-- could look unpinned).
local function sweep()
	local days = M.archive_after:peek()
	local now_state = sessions.now:peek()
	if now_state.state ~= "ready" or not now_state.places_ready then
		return
	end
	local t = os.time()
	local archived: { [string]: boolean } = {}
	for _, c in now_state.archived or {} do
		archived[c.id] = true
	end
	local was = seen_archived
	seen_archived = archived
	local came_back: { [string]: number } = table.clone(back:peek())
	local changed = false
	local here: { [string]: boolean } = {}
	for _, c in now_state.chats do
		here[c.id] = true
		if was and was[c.id] then
			came_back[c.id] = t
			changed = true
		end
	end
	for id in came_back do
		if not here[id] then
			came_back[id] = nil
			changed = true
		end
	end
	if changed then
		back:set(came_back)
	end
	-- (The first sweep after a launch only looks: what came back meanwhile
	-- is unknown until the next. Turned off, it only looks too.)
	if not was or not M.on:peek() or type(days) ~= "number" or days <= 0 then
		return
	end
	local pinned: { [string]: boolean } = {}
	for _, id in now_state.places.pinned do
		pinned[id] = true
	end
	local open = route.chat:peek().session
	local cutoff = t - days * 86400
	for _, c in now_state.chats do
		local st = c.status
		local idle_since = math.max(c.updated or 0, came_back[c.id] or 0)
		if
			not pinned[c.id]
			and c.id ~= open
			and not c.sleep_until
			and not c.woke
			and not (st and (UNDER_WAY[st] or NEEDS_YOU[st]))
			and idle_since > 0
			and idle_since < cutoff
		then
			sessions.archive(c.id)
		end
	end
end

-- The minute beat: one timer for the module, armed here and not in an
-- effect (an effect owns the timers it arms, and every run of one would
-- cancel and restart it: a busy minute of session changes would starve it).
-- Each tick does something only while something reads it: a sleeper's
-- countdown moves while one is asleep, and the sweep runs while
-- auto-archive is on. A reload numbers the beat anew; the old one stops.
local beat_gen = ui.state("activity_beat_gen", 0)
beat_gen:set(beat_gen:peek() + 1)
local my_beat = beat_gen:peek()
local function beat()
	ui.after(60 * 1000, function()
		if beat_gen:peek() ~= my_beat then
			return
		end
		if M.on:peek() then
			for _, c in sessions.now:peek().chats do
				if c.sleep_until then
					minute:set(minute:peek() + 1)
					break
				end
			end
			sweep()
		end
		beat()
	end)
end
beat()
-- (Auto-archive turned on, or its days changed: a sweep now, not a minute
-- on.)
ui.effect(function()
	M.on:get()
	M.archive_after:get()
	ui.after(0, sweep)
end)

return M

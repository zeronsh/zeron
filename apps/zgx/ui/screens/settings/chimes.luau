-- The sessions' chimes, as the shipping app plays them (crates/ui
-- shell.rs, its one session detector; crates/engine desktop_notify's
-- `ping_since`): a chat waiting on you, or failed, plays its sound as its
-- status turns so; a turn finished plays "done", if Settings →
-- Notifications has it on. A chat's first appearance only sets where it
-- stands (a launch, a sync, a hot reload never ring), and what it was is
-- kept in state, so a reload picks up where it was.
--
-- "Finished" is the turn mark moving (`turn`, the session's
-- `lastCompletedTurn`), never the status "done": that's only activity you
-- haven't seen, and your own message makes it too (issue #1437). Nor does
-- every move ring: one published with the next turn already running is
-- your message carrying it on (a steer, a queued message taken), and a
-- send that replaces the agent's runtime marks the old turn's end a moment
-- before your turn starts, so a finish waits `SETTLE` to stand; a turn
-- going on (or asking, or failing) meanwhile takes it with it.
--
-- And the window's focus, told to the engine (`SetViewportFocus`), which
-- holds its banners while Zeron is the app in front if Notifications says
-- "Only when in the background". Told when it changes, then again every 20
-- seconds while it holds (the engine forgets a report that goes stale).

local host = require("@app/screens/settings/host")
local sessions = require("@account/sessions")
local prefs = require("@app/screens/settings/prefs")

local M = {}

type Seen = { status: string, turn: string? }

-- Each chat as last seen, by id: its status ("" for idle) and finished turn.
local seen = ui.state("settings_chimes_seen_turns", {} :: { [string]: Seen })

type Wait = { turn: string, at: number, by: number }

-- Finishes waiting to stand, by chat: the turn, when it settles (`at`),
-- the latest it rings (`by`, prefetched or not) — issue #1438: the chime
-- means "go read it now", so it follows the tail's prefetch, bounded.
local settling = ui.state("settings_chimes_settling", {} :: { [string]: Wait })

-- The chats whose tail's prefetch has landed (its own state: a prefetch
-- that settles while the settle is still being written can't be lost,
-- and the timer below wakes on it).
local prefetched = ui.state("settings_chimes_prefetched", {} :: { [string]: boolean })

-- How long a finish waits to stand before it rings (the engine's banners'
-- `DONE_SETTLE`).
local SETTLE = 2000

-- And the most it waits on the tail's prefetch past that: a slow or
-- offline engine still rings rather than swallowing the finish.
local PREFETCH_WAIT = 4000

-- What a status turning on plays. ("done" isn't one: see above.)
local SOUND: { [string]: string } = {
	input = "request",
	failed = "attention",
}

-- A turn going on, or asking, or failing: nothing has finished.
local function busy(status: string): boolean
	return status == "working" or status == "waiting" or status == "input" or status == "failed"
end

-- One chime for many at once: the most urgent.
local function louder(ring: string?, sound: string): string
	if ring == nil or sound == "attention" or (sound == "request" and ring == "done") then
		return sound
	end
	return ring
end

local function play(ring: string?)
	if ring and prefs.chimes(ring) then
		host.invoke("sound.play", ring)
	end
end

ui.effect(function()
	local now = sessions.now:get()
	if now.state ~= "ready" then
		return
	end
	local before = seen:peek()
	local after: { [string]: Seen } = {}
	local waits = table.clone(settling:peek())
	local moved = false
	local ring: string? = nil
	for _, c in now.chats do
		local status = c.status or ""
		local was = before[c.id]
		-- (A session row gone for a while, a host restarting, keeps the turn
		-- last seen: its coming back is no finish.)
		after[c.id] = { status = status, turn = c.turn or (was and was.turn) }
		if busy(status) and waits[c.id] then
			waits[c.id] = nil
			moved = true
		end
		if was ~= nil then
			if was.status ~= status and SOUND[status] then
				ring = louder(ring, SOUND[status])
			end
			if c.turn ~= nil and c.turn ~= was.turn and c.turn_fresh ~= false and not busy(status) then
				waits[c.id] = { turn = c.turn, at = ui.now() + SETTLE, by = ui.now() + SETTLE + PREFETCH_WAIT }
				moved = true
				local id = c.id
				prefetched:set_key({ id }, nil)
				-- The chat's tail ahead of time (a no-op where it's already
				-- open): when it's in hand, or can't be, the finish may ring.
				host.call("chat.prefetch", { id = id }):after(function()
					prefetched:set_key({ id }, true)
				end, function()
					prefetched:set_key({ id }, true)
				end)
			end
		end
	end
	-- (A chat gone takes its finish with it.)
	for id in waits do
		if after[id] == nil then
			waits[id] = nil
			moved = true
		end
	end
	seen:set(after)
	if moved then
		settling:set(waits)
	end
	play(ring)
end)

-- The finishes due by `upto` have stood: they ring, once each moment (one
-- chime for many). (The timer that calls this is the deadline: its clock
-- needn't be `ui.now()`'s, so that's never asked again here.)
local function stand(upto: number)
	local waits = settling:peek()
	local left = {}
	local ring: string? = nil
	local warm = prefetched:peek()
	for id, w in waits do
		if w.at <= upto then
			if warm[id] or w.by <= upto then
				local now = seen:peek()[id]
				if now and now.turn == w.turn and not busy(now.status) then
					ring = louder(ring, "done")
				end
			else
				left[id] = w
			end
		else
			left[id] = w
		end
	end
	settling:set(left)
	play(ring)
end
ui.effect(function()
	local now = ui.now()
	local warm = prefetched:get()
	local dues: { [number]: boolean } = {}
	for id, w in settling:get() do
		-- Its looks: the settle time, and — while the tail isn't in hand —
		-- the prefetch's bound, so a tail that never lands still rings
		-- there. Once it is, the settle time alone decides (the effect runs
		-- again as it lands).
		dues[w.at] = true
		if not warm[id] then
			dues[w.by] = true
		end
	end
	-- (A timer made in an effect belongs to its run: a new finish, or one
	-- taken, sets them again.)
	for due in dues do
		local upto: number = due
		ui.after(math.max(0, upto - ui.now()), function()
			stand(upto)
		end)
	end
end)

-- The window's focus, as last told (nil: not yet).
local VIEWPORT = "zgx-main"
local told = ui.state("settings_focus_told", nil :: boolean?)
local function tell(focused: boolean)
	told:set(focused)
	host.call("settings.engine", { method = "SetViewportFocus", params = { viewportId = VIEWPORT, focused = focused } })
		:after(function() end, function() end)
end
ui.effect(function()
	local active = ui.window.active:get()
	if active ~= told:peek() then
		tell(active)
	end
	-- Focused, said again every 20 s: the engine lets a report go stale
	-- (crates/engine/src/desktop_notify.rs, `TTL`). Only while focused (a
	-- timer made in an effect stops as it runs again), so an unfocused
	-- window has no timer at all.
	if active then
		ui.every(20000, function()
			tell(true)
			return true
		end)
	end
end)

return M

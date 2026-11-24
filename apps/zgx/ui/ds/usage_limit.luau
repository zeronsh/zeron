--!strict
-- A thread its provider's usage limit stopped (its session row's `limited`,
-- src/orchestration.rs `limited_value`): waiting for the reset ("Waiting
-- for limit", "resets 14:05", local time; the first alone while the reset
-- isn't known), or running meanwhile on another account ("On me@home.dev",
-- "me@work.dev resets 14:05"). Pure but for the locale's words:
-- every place a thread's status shows reads it the same.
--
--   limit.of(chat) -> Limit?        its limit, if any
--   limit.waiting(chat) -> boolean  limited, and not switched
--   limit.parts(l) -> { string }    the facts above, apart, to space
--   limit.text(l) -> string         those as one phrase, for a label
--   limit.left(l, now) -> string    "resets in 12m" (none once it's past
--                                   or unknown), for a clock that already
--                                   ticks
--   limit.clock(t) -> "14:05", limit.local_at(day, 14, 5) -> Unix seconds

local locale = require("@ds/locale")

local M = {}

export type Switched = { account_slot: string?, account_label: string?, at: number?, harness: string?, model: string? }
export type Limit = {
	provider: string?,
	account_slot: string?,
	account_label: string?,
	window: string?,
	until_at: number?,
	since: number?,
	attempt: number?,
	switched_to: Switched?,
}

function M.of(chat: any): Limit?
	local l = if type(chat) == "table" then chat.limited else nil
	return if type(l) == "table" then l :: Limit else nil
end

function M.switched(l: Limit?): boolean
	return l ~= nil and type(l.switched_to) == "table"
end

-- Limited and still waiting on its own account.
function M.waiting(chat: any): boolean
	local l = M.of(chat)
	return l ~= nil and not M.switched(l)
end

-- A clock time, local, on a 24-hour clock: "14:05".
function M.clock(at: number): string
	local d = os.date("*t", math.floor(at)) :: any
	return string.format("%02d:%02d", d.hour, d.min)
end

-- `hour`:`min` local on the day `day` (Unix seconds) falls in. (Luau's
-- `os.time` reads a table as UTC: the zone's offset is taken back out, as
-- screens/shell/activity's `local_time` does.)
function M.local_at(day: number, hour: number, min: number): number
	local d = os.date("*t", math.floor(day)) :: any
	local as_utc = os.time({ year = d.year, month = d.month, day = d.day, hour = hour, min = min, sec = 0 } :: any)
	local function offset(t: number): number
		return os.time(os.date("*t", t) :: any) - t
	end
	local guess = as_utc - offset(as_utc)
	return as_utc - offset(guess)
end

local function known_until(l: Limit): number?
	return if type(l.until_at) == "number" and l.until_at > 0 then l.until_at else nil
end

-- Its facts apart, each a phrase of its own ("Waiting for limit", "resets
-- 14:05"; "On me@home.dev", "me@work.dev resets 14:05"), for a line that
-- spaces them: no separator is ever typed between them.
function M.parts(l: Limit?): { string }
	if not l then
		return {}
	end
	local reset = known_until(l)
	local to = l.switched_to
	if type(to) == "table" then
		local account = to.account_label or to.account_slot or ""
		local original = l.account_label or l.provider or locale.s("ds.limit.original")
		return {
			locale.s("ds.limit.on", { account = account }),
			if reset
				then locale.s("ds.limit.original_resets", { original = original, time = M.clock(reset) })
				else locale.s("ds.limit.original_limited", { original = original }),
		}
	end
	local out = { locale.s("ds.limit.waiting") }
	if reset then
		table.insert(out, locale.s("ds.limit.resets", { time = M.clock(reset) }))
	end
	return out
end

-- Those facts as one phrase, for what reads it as a sentence (a label):
-- "Waiting for limit, resets 14:05".
function M.text(l: Limit?): string
	local out: string? = nil
	for _, t in M.parts(l) do
		out = if out then locale.s("ds.limit.and_then", { first = out, rest = t }) else t
	end
	return out or ""
end

-- How long until it resets, to the minute ("resets in 12m"); "" when the
-- reset isn't known or has passed.
function M.left(l: Limit?, now: number): string
	local reset = if l then known_until(l) else nil
	if not reset or reset <= now then
		return ""
	end
	local t = reset - now
	local words: string
	if t < 60 then
		words = locale.s("ds.orch.progress.under_minute")
	elseif t < 3600 then
		words = locale.s("time.short.minutes", { n = t // 60 })
	else
		local m = t // 60
		words = if m % 60 == 0 then locale.s("time.short.hours", { n = m // 60 }) else locale.s("time.short.hours_minutes", { h = m // 60, m = m % 60 })
	end
	return locale.s("ds.limit.resets_in", { t = words })
end

return M

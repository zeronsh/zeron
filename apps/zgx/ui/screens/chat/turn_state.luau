-- What a turn's foot says while its reply is unfinished: the working word, or,
-- when there's no turn to speak of, why. A working word is only true while the
-- chat's host is there and its turn is heard from (the session's `running`,
-- which goes stale with its host); a reply the host never finished stays
-- "streaming" in the transcript for ever, and its word would spin on.
--
--   TurnState.of { streaming = true, live = false, started = t, now = os.time(),
--     cloud = session?, host_unreachable = false, host = "Build box" }
--     → nil (its working word) | { text = "Interrupted", spinner? }
--
-- Pure: a test requires it alone, in a VM with no `ui`, and reads the English.

local M = {}

local function say(key: string, english: string, args: { [string]: any }?): string
	if (ui :: any) == nil then
		return english
	end
	return ui.t(`chat.turn_state.{key}`, args):get()
end

-- A reply just begun may be heard of before its session says the turn's
-- running: its word stands this long before it's doubted.
M.GRACE = 15

export type State = { text: string, spinner: boolean? }
export type Turn = {
	-- An unfinished reply (or a prompt whose reply hasn't begun).
	streaming: boolean,
	-- The session says a turn runs, and was heard from lately.
	live: boolean,
	-- When the reply began, and now (Unix seconds).
	started: number?,
	now: number,
	-- The chat's Cloud session, when it runs on one.
	cloud: any?,
	-- Its host is another computer, and that's gone quiet.
	host_unreachable: boolean?,
	host: string?,
	-- A prompt whose reply hasn't begun, on its way.
	sending: boolean?,
}

local function interrupted(why: string?): State
	if why then
		return { text = why }
	end
	return { text = say("interrupted", "Interrupted") }
end

function M.of(t: Turn): State?
	if not t.streaming then
		return nil
	end
	local s = t.cloud
	if s then
		local state = s.state
		local starting = state == "starting" or state == "provisioning"
		-- A prompt on its way to a machine still waking: that it's waking.
		if t.sending then
			return if starting then { text = say("waking", "Waking its Cloud machine…"), spinner = true } else nil
		end
		if state == "data_expired" then
			return interrupted(say("expired", "Interrupted — its Cloud machine's files are gone"))
		elseif s.wake_blocked and s.wake_blocked.reason == "disk_too_small" then
			return interrupted(say("blocked", "Interrupted — its Cloud machine can't wake"))
		elseif state == "error" then
			return interrupted(say("failed", "Interrupted — its Cloud machine failed"))
		elseif state == "sleeping" or state == "stopping" then
			return interrupted(say("asleep", "Interrupted — its Cloud machine went to sleep"))
		elseif starting then
			-- (A machine that died mid-turn is woken and picks its turn up.)
			return { text = say("resuming", "Resuming — waking its Cloud machine…"), spinner = true }
		elseif state == "deleting" or state == "off" then
			return interrupted()
		end
	elseif t.host_unreachable then
		if t.sending then
			return nil
		end
		local host = t.host or say("the_host", "its computer")
		return interrupted(say("unreachable", `Interrupted — can't reach {host}`, { host = host }))
	end
	if t.sending or t.live then
		return nil
	end
	if t.started and t.now - t.started < M.GRACE then
		return nil
	end
	return interrupted()
end

return M

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

-- `step`: a send's Cloud machine starting ("start") or waking ("wake"), said
-- as the machine's own first setup step, a tool row under the message
-- (`summary`, its group's line), until the machine's own steps replace it.
export type State = { text: string, spinner: boolean?, step: string?, summary: string? }
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
	-- Its Cloud machine's wake failed (the deck's alert says why).
	wake_failed: boolean?,
	-- The prompt is still in the engine's outbox, its files uploading:
	-- nothing has gone to the machine yet.
	held: boolean?,
	-- The reply is its machine's own setup steps (`<message>.cloud-setup`):
	-- they say how its machine is coming up themselves.
	setup: boolean?,
}

-- A stage the edge names, in words (as screens/chat/cloud_machine's line).
local function stage_words(stage: string?): string?
	if stage == "clone" then
		return say("stage_clone", "cloning the repository")
	elseif stage == "branch" then
		return say("stage_branch", "checking out the branch")
	elseif stage and string.find(stage, " ", 1, true) then
		return stage
	end
	return nil
end

-- A send's machine coming up, as its first setup step: at once, before the
-- machine says it's starting (asleep, it wakes for the send; not yet known,
-- a new chat's is starting), and with its stage as it goes.
local function step(s: any): State?
	local state = s.state
	local up = state == "ready"
	local starting = state == "starting" or state == "provisioning"
	-- (One that can't wake holds the send, and the deck says why.)
	if s.wake_blocked then
		return nil
	end
	local which = if state == "provisioning" or s.unknown then "start" else "wake"
	local function as_step(text: string): State
		return {
			text = text,
			spinner = true,
			step = which,
			summary = if which == "start"
				then say("setup_summary", "Set up the Cloud machine")
				else say("woke_summary", "Woke the Cloud machine"),
		}
	end
	if s.stage == "updating" and (up or starting) then
		return as_step(say("updating", "Updating its Cloud machine…"))
	elseif s.stage == "slow" and (up or starting) then
		return as_step(say("slow", "Waking its Cloud machine is taking longer than usual…"))
	elseif starting then
		local stage = stage_words(s.stage)
		if stage then
			return as_step(say("waking_stage", `Waking its Cloud machine — {stage}…`, { stage = stage }))
		end
		return as_step(if which == "start"
			then say("starting", "Starting a Cloud machine…")
			else say("waking", "Waking its Cloud machine…"))
	elseif s.unknown then
		return as_step(say("starting", "Starting a Cloud machine…"))
	elseif state == "sleeping" or state == "stopping" then
		return as_step(say("waking", "Waking its Cloud machine…"))
	end
	return nil
end

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
		-- Up, and installing a release before it answers (the edge's stage:
		-- said only once its engine is up, and taken back when it's done).
		local updating = s.stage == "updating" and (state == "ready" or starting)
		-- A wake far slower than normal, the edge kicking it meanwhile.
		local slow = s.stage == "slow" and (state == "ready" or starting)
		-- A prompt on its way to a machine coming up: that it is, as its
		-- first setup step. (A wake that failed says why in the deck.)
		if t.sending then
			return if t.wake_failed or t.held then nil else step(s)
		end
		if t.setup and (starting or updating or slow) then
			return nil
		end
		if state == "data_expired" then
			return interrupted(say("expired", "Interrupted — its Cloud machine's files are gone"))
		elseif s.wake_blocked and s.wake_blocked.reason == "disk_too_small" then
			return interrupted(say("blocked", "Interrupted — its Cloud machine can't wake"))
		elseif state == "error" then
			return interrupted(say("failed", "Interrupted — its Cloud machine failed"))
		elseif state == "sleeping" or state == "stopping" then
			return interrupted(say("asleep", "Interrupted — its Cloud machine went to sleep"))
		elseif updating then
			-- (Woken for a turn it was cut off in, it updates before it picks
			-- the turn up.)
			return { text = say("resuming_updating", "Resuming — updating its Cloud machine…"), spinner = true }
		elseif slow then
			return { text = say("resuming_slow", "Resuming — its Cloud machine is taking longer than usual to wake…"), spinner = true }
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

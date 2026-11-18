-- The pure part of ds/orch_progress (no requires, for tests to read): a
-- progress row's state in Zeron's status words, its clocks at a moment
-- (the contract's: elapsed = now − started, stopped at `updated` once over;
-- since the last step = now − last_step; time left = eta_secs − (now −
-- updated), only while working, none when none is left), the minute a
-- moment falls in, and what it is on.

local M = {}

export type Row = { [string]: any }

-- A row's state in Zeron's status words ("idle": none to show). A thread
-- that asked the person and waits on the answer (`asking`) awaits input,
-- however its turn ended.
local KIND: { [string]: string } = {
	working = "working",
	waiting = "waiting",
	awaiting_input = "input",
	failed = "failed",
	done = "done",
}
-- Still on it: its clock runs.
M.ACTIVE = { working = true, waiting = true, awaiting_input = true } :: { [string]: boolean }

function M.kind(r: Row?): string
	if r and r.asking == true then
		return "input"
	end
	return (r and KIND[r.state or ""]) or "idle"
end

-- Working and seemingly stuck.
function M.stalled(r: Row?): boolean
	return r ~= nil and r.stalled == true and r.state == "working"
end

export type Clocks = { elapsed: number?, since_step: number?, tool: number?, left: number? }

-- The clocks at `now` (Unix seconds), each nil when the row can't say.
function M.clocks(r: Row?, now: number): Clocks
	local out: Clocks = {}
	if not r then
		return out
	end
	local active = M.ACTIVE[r.state or ""] == true
	if type(r.started) == "number" then
		-- (Over: stopped where it ended.)
		local stop = if active then now else (r.updated or now)
		out.elapsed = math.max(0, stop - r.started)
	end
	if type(r.last_step) == "number" then
		out.since_step = math.max(0, now - r.last_step)
	end
	if r.state == "working" and type(r.tool_since) == "number" then
		out.tool = math.max(0, now - r.tool_since)
	end
	if r.state == "working" and type(r.eta_secs) == "number" then
		local left = r.eta_secs - (now - (r.updated or now))
		if left > 0 then
			out.left = left
		end
	end
	return out
end

-- What it is on this moment: the command running (the session row's live
-- label for it), else, working, what its agent is doing; nil when the row
-- says nothing more than its state.
function M.doing(r: Row?, chat: { [string]: any }?): string?
	if not r or r.state ~= "working" then
		return nil
	end
	local a = chat and chat.activity
	local live = if type(a) == "table" and type(a.label) == "string" and a.label ~= "" then a.label else nil
	if r.tool and r.tool ~= "" then
		return live or (r.tool :: string)
	end
	return live
end

-- The todo item under way (not the command: that's `doing`).
function M.step(r: Row?): string?
	if not r or not r.step or r.step == "" or r.step == r.tool then
		return nil
	end
	return r.step :: string
end

-- The minute `t` (Unix seconds) falls in, as its first second; and how
-- many seconds till the next one begins (1–60).
function M.minute(t: number): number
	return math.floor(t) - math.floor(t) % 60
end

function M.until_minute(t: number): number
	return 60 - math.floor(t) % 60
end

return M

-- A decision an orchestrator or its crew asked of the person, as every
-- place that offers it reads it: Needs you's row and a board card waiting
-- on it (both compact: the question in a line, its options as chips) and
-- the overlay that shows the whole of it (ds/orch_decision).
--
-- A pick is sent after a moment it can be taken back in: it waits here
-- (`pending`) for UNDO_MS, every place showing "Sending, Undo", and only
-- then is it sent. Undo drops it. The overlay's Decide goes the same way,
-- so the row and the card say it too. Only once the engine has taken it
-- does it say "Answered" (for ANSWERED_MS), and then it leaves Needs you;
-- if the engine turns it down (or doesn't reply), the pick is undone and
-- the ask says why, with Retry (`failed`). Waiting, sending or answered,
-- it isn't counted as needing you.
--
--   decide.options(ask)          its options, 1-based: { index, label,
--                                detail?, images, scores, recommended }
--   decide.recommended(ask)      the recommended option's index, if any
--   decide.why(ask)              why it is recommended, if it says
--   decide.images(ask)           the ask's own pictures (not an option's)
--   decide.deferred(ask, now?)   deferred past now (seconds)
--   decide.pick(ask, choice, commit)  choice { option?, text? }; commit(ask,
--                                choice) after UNDO_MS unless undone: a
--                                promise of the engine's reply
--   decide.undo(id), decide.pending_of(id), decide.failed_of(id)
--   decide.retry(id)             a failed answer (or defer) sent again
--   decide.defer(ask, until_ms, send)  send(ask, until_ms): a promise; a
--                                failure said as an answer's is
--   decide.counted(ask_id)       whether it counts as needing you
--   decide.listed(ask_id)        whether Needs you lists it (not once
--                                answered here, until STALE_MS: the engine
--                                says so by then, or it was never taken)
--   decide.recent(asks, limit)   the latest answered: { id, question,
--                                answer, at }, newest first
--   decide.show(id), decide.hide(), decide.shown   the overlay's ask
--   decide.defer_times(now)      the Defer menu's choices: { key, at } (ms)

local data = require("@self/orch_data")
local locale = require("@self/locale")

local M = {}

-- How long a pick can be taken back before it is sent.
M.UNDO_MS = 5000
-- How long "Answered" shows once the engine has taken it.
M.ANSWERED_MS = 2500
-- How long an ask answered here stays out of Needs you while the engine
-- still lists it open (its state catching up); past it, it shows again.
M.STALE_MS = 30000

export type Choice = { option: number?, text: string? }
-- `phase`: "waiting" (its Undo window), "sending" (sent, no reply yet),
-- "answered" (the engine took it: said for ANSWERED_MS) and "done" (gone
-- from Needs you). `sent`: past its Undo window. `at`: when the engine
-- took it (seconds).
export type Phase = "waiting" | "sending" | "answered" | "done"
export type Pending = {
	option: number?,
	text: string?,
	label: string,
	question: string?,
	phase: Phase,
	sent: boolean?,
	at: number?,
}
-- What the engine turned down (or never replied to): an answer or a
-- defer, and why.
export type Failure = { kind: "answer" | "defer", reason: string, label: string? }

-- Picks waiting, on their way or just answered, by ask id.
M.pending = ui.state("orch_decide_pending", {} :: { [string]: Pending })
-- The asks an answer or a defer failed for, by ask id.
M.failed = ui.state("orch_decide_failed", {} :: { [string]: Failure })
-- Each failure's Retry.
local retries: { [string]: () -> () } = {}
-- The ask the overlay shows (its id), nil while closed.
M.shown = ui.state("orch_decide_shown", nil :: string?)
-- Its commits, by ask id: cancelled by Undo.
local timers: { [string]: () -> () } = {}

-- An option's scores as a list, in the order given (the bridge's: a list
-- of { name, value }); a map, by name.
local function scores_of(s: any): { { name: string, value: string } }
	local out: { { name: string, value: string } } = {}
	if type(s) ~= "table" then
		return out
	end
	if #s > 0 then
		for _, e in s do
			if type(e) == "table" and e.name ~= nil then
				table.insert(out, { name = tostring(e.name), value = tostring(e.value or "") })
			end
		end
		return out
	end
	for k, v in s do
		table.insert(out, { name = tostring(k), value = if type(v) == "number" and v == math.floor(v) then tostring(math.floor(v)) else tostring(v) })
	end
	table.sort(out, function(a: { name: string, value: string }, b: { name: string, value: string }): boolean
		return a.name < b.name
	end)
	return out
end

-- The recommended option's index (1-based, as the bridge gives it), else
-- the option its label names.
function M.recommended(ask: any): number?
	local r = ask and ask.recommendation
	local opts = (ask and ask.options) or {}
	if type(r) ~= "table" then
		return nil
	end
	if type(r.option) == "number" and opts[r.option] then
		return r.option
	end
	if type(r.label) == "string" and r.label ~= "" then
		for i, o in opts do
			if o.label == r.label then
				return i
			end
		end
	end
	return nil
end

function M.images(ask: any): { string }
	local out: { string } = {}
	for _, src in (ask and ask.images) or {} do
		if type(src) == "string" and src ~= "" then
			table.insert(out, src)
		end
	end
	return out
end

function M.why(ask: any): string?
	local r = ask and ask.recommendation
	local w = if type(r) == "table" then r.why else nil
	return if type(w) == "string" and w ~= "" then w else nil
end

export type Option = {
	index: number,
	label: string,
	detail: string?,
	images: { string },
	scores: { { name: string, value: string } },
	recommended: boolean,
}

function M.options(ask: any): { Option }
	local out: { Option } = {}
	local rec = M.recommended(ask)
	for i, o in (ask and ask.options) or {} do
		local images: { string } = {}
		for _, src in o.images or {} do
			if type(src) == "string" and src ~= "" then
				table.insert(images, src)
			end
		end
		table.insert(out, {
			index = i,
			label = o.label or "",
			detail = if o.detail and o.detail ~= "" then o.detail else nil,
			images = images,
			scores = scores_of(o.scores),
			recommended = i == rec,
		})
	end
	return out
end

M.until_of = data.deferred_until
M.deferred = data.deferred

-- Unix milliseconds as the wire's RFC3339 (UTC).
function M.iso(ms: number): string
	return os.date("!%Y-%m-%dT%H:%M:%SZ", math.floor(ms / 1000)) :: string
end

-- The Defer menu's choices from `now` (seconds): in an hour, in four,
-- tomorrow at nine, next Monday at nine. `at` in milliseconds.
function M.defer_times(now: number?): { { key: string, at: number } }
	local t = now or os.time()
	-- (9:00 here, `days_on` days on. Not through os.time of a table: zgx
	-- reads one as UTC, which put "9:00" at 9:00 UTC.)
	local function nine(days_on: number): number
		local d = os.date("*t", t) :: any
		local at = t - (d.hour * 3600 + d.min * 60 + d.sec) + days_on * 86400 + 9 * 3600
		-- (A clock change on the way: back on 9.)
		local h = (os.date("*t", at) :: any).hour
		return at + (9 - h) * 3600
	end
	local wday = (os.date("*t", t) :: any).wday -- 1 Sunday … 7 Saturday
	local to_monday = (9 - wday) % 7
	if to_monday == 0 then
		to_monday = 7
	end
	return {
		{ key = "hour", at = (t + 3600) * 1000 },
		{ key = "hours", at = (t + 4 * 3600) * 1000 },
		{ key = "tomorrow", at = nine(1) * 1000 },
		{ key = "week", at = nine(to_monday) * 1000 },
	}
end

-- A time to put an ask off until, as its menu says it: "For an hour",
-- "Until tomorrow, 9:00 AM" (the hour as every time is said: ds/locale).
function M.defer_label(t: { key: string, at: number }): string
	return locale.s(`ds.orch.decide.defer_{t.key}`, { time = locale.clock(t.at / 1000) })
end

-- What a choice says, answered: its option's label and any words.
function M.label_of(ask: any, choice: Choice): string
	local o = if choice.option then ((ask and ask.options) or {})[choice.option] else nil
	local text = choice.text and string.match(choice.text, "^%s*(.-)%s*$") or ""
	if o and text ~= "" then
		return `{o.label}: {text}`
	end
	return if o then o.label or "" else text
end

function M.pending_of(id: string?): Pending?
	return if id then M.pending:get()[id] else nil
end

function M.failed_of(id: string?): Failure?
	return if id then M.failed:get()[id] else nil
end

function M.counted(ask_id: string): boolean
	return M.pending:get()[ask_id] == nil
end

function M.listed(ask_id: string): boolean
	local p = M.pending:get()[ask_id]
	return p == nil or p.phase ~= "done"
end

-- What went wrong, in words: a promise's error (a `UiError`'s `message`),
-- what a call raised (its script position gone), or the timeout.
function M.reason(err: any): string
	if type(err) == "string" then
		return (string.gsub(err, "^.-:%d+: ", ""))
	end
	if type(err) == "table" or type(err) == "userdata" then
		local ok, e = pcall(function(): any
			return { kind = (err :: any).kind, message = (err :: any).message }
		end)
		if ok then
			if e.kind == "timeout" then
				return locale.s("ds.orch.decide.timeout")
			end
			if type(e.message) == "string" and e.message ~= "" then
				return (string.gsub(e.message, "^.-:%d+: ", ""))
			end
		end
	end
	return tostring(err)
end

local function set_phase(id: string, phase: Phase, at: number?)
	local p = M.pending:peek()[id]
	if p then
		local q = table.clone(p)
		q.phase = phase
		q.sent = phase ~= "waiting"
		q.at = at or q.at
		M.pending:set_key({ id }, q)
	end
end

-- A pick sent: "Answered" once the engine takes it, then out of Needs
-- you; turned down, undone, the failure said.
local function send(ask: any, c: Choice, commit: (ask: any, choice: Choice) -> any)
	local id = ask.id
	local p = M.pending:peek()[id]
	if not p then
		M.pending:set_key({ id }, {
			option = c.option,
			text = c.text,
			label = M.label_of(ask, c),
			question = ask.question,
			phase = "sending",
			sent = true,
		})
	else
		set_phase(id, "sending")
	end
	M.failed:set_key({ id }, nil)
	retries[id] = nil
	local function taken()
		local q = M.pending:peek()[id]
		if not q or q.phase ~= "sending" then
			return
		end
		set_phase(id, "answered", os.time())
		ui.after(M.ANSWERED_MS, function()
			local r = M.pending:peek()[id]
			if r and r.phase == "answered" then
				set_phase(id, "done")
				ui.after(M.STALE_MS, function()
					local d = M.pending:peek()[id]
					if d and d.phase == "done" then
						M.pending:set_key({ id }, nil)
					end
				end)
			end
		end)
	end
	local function failed(err: any)
		local q = M.pending:peek()[id]
		ui.batch(function()
			M.pending:set_key({ id }, nil)
			M.failed:set_key({ id }, { kind = "answer", reason = M.reason(err), label = q and q.label })
		end)
		retries[id] = function()
			send(ask, c, commit)
		end
	end
	local ok, promise = pcall(commit, ask, c)
	if not ok then
		failed(promise)
	elseif type(promise) == "table" or type(promise) == "userdata" then
		promise:after(taken, failed)
	else
		-- (A commit with no promise: taken as it is.)
		taken()
	end
end

-- A pick: shown at once, sent after UNDO_MS unless undone. One pick an
-- ask; a second while one waits does nothing.
function M.pick(ask: any, choice: Choice, commit: (ask: any, choice: Choice) -> any)
	local id = ask and ask.id
	if not id or M.pending:peek()[id] then
		return
	end
	local text = choice.text and string.match(choice.text, "^%s*(.-)%s*$") or nil
	if text == "" then
		text = nil
	end
	if not choice.option and not text then
		return
	end
	local c: Choice = { option = choice.option, text = text }
	ui.batch(function()
		M.failed:set_key({ id }, nil)
		M.pending:set_key({ id }, {
			option = c.option,
			text = c.text,
			label = M.label_of(ask, c),
			question = ask.question,
			phase = "waiting",
		})
	end)
	retries[id] = nil
	timers[id] = ui.after(M.UNDO_MS, function()
		timers[id] = nil
		if M.pending:peek()[id] then
			send(ask, c, commit)
		end
	end)
end

function M.undo(id: string)
	local p = M.pending:peek()[id]
	if p and p.phase ~= "waiting" then
		return
	end
	local cancel = timers[id]
	if cancel then
		cancel()
		timers[id] = nil
	end
	M.pending:set_key({ id }, nil)
end

-- A failed answer (or defer) sent again, at once.
function M.retry(id: string)
	local again = retries[id]
	retries[id] = nil
	if again then
		again()
	end
end

-- An ask put off: `send(ask, until_ms)`, a promise. Turned down, said on
-- the ask as an answer's failure is, with Retry.
function M.defer(ask: any, until_ms: number, send_defer: (ask: any, until_ms: number) -> any)
	local id = ask and ask.id
	if not id then
		return
	end
	M.failed:set_key({ id }, nil)
	retries[id] = nil
	local function failed(err: any)
		M.failed:set_key({ id }, { kind = "defer", reason = M.reason(err) })
		retries[id] = function()
			M.defer(ask, until_ms, send_defer)
		end
	end
	local ok, promise = pcall(send_defer, ask, until_ms)
	if not ok then
		failed(promise)
	elseif type(promise) == "table" or type(promise) == "userdata" then
		promise:after(function() end, failed)
	end
end

-- The engine's words for an answer to an option ("Option 2: Rebuild now
-- — …") as the person's: the label and words.
local function said(answer: string): string
	return (string.gsub(answer, "^Option %d+: ", ""))
end

-- The latest answered asks, newest first: what the engine says was
-- answered, and what it took here and hasn't said yet.
function M.recent(asks: { any }?, limit: number?): { { id: string, question: string, answer: string, at: number } }
	local out: { { id: string, question: string, answer: string, at: number } } = {}
	local seen: { [string]: boolean } = {}
	for _, a in asks or {} do
		if a.state == "answered" and type(a.id) == "string" then
			seen[a.id] = true
			table.insert(out, { id = a.id, question = a.question or "", answer = said(a.answer or ""), at = a.answered or 0 })
		end
	end
	for id, p in M.pending:get() do
		if not seen[id] and (p.phase == "answered" or p.phase == "done") then
			table.insert(out, { id = id, question = p.question or "", answer = p.label, at = p.at or os.time() })
		end
	end
	table.sort(out, function(a: any, b: any): boolean
		if a.at ~= b.at then
			return a.at > b.at
		end
		return a.id < b.id
	end)
	local n = limit or 5
	while #out > n do
		table.remove(out)
	end
	return out
end

function M.show(id: string)
	M.shown:set(id)
end

function M.hide()
	M.shown:set(nil)
end

return M

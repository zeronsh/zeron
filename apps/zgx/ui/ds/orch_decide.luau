-- A decision an orchestrator or its crew asked of the person, as every
-- place that offers it reads it: Needs you's row and a board card waiting
-- on it (both compact: the question in a line, its options as chips) and
-- the overlay that shows the whole of it (ds/orch_decision).
--
-- A pick is answered at once and can be taken back for a moment: it waits
-- here (`pending`) for UNDO_MS, every place showing "Answered · Undo", and
-- only then is it sent. Undo drops it. The overlay's Decide goes the same
-- way, so the row and the card say it too.
--
--   decide.options(ask)          its options, 1-based: { index, label,
--                                detail?, images, scores, recommended }
--   decide.recommended(ask)      the recommended option's index, if any
--   decide.why(ask)              why it is recommended, if it says
--   decide.images(ask)           the ask's own pictures (not an option's)
--   decide.deferred(ask, now?)   deferred past now (seconds)
--   decide.pick(ask, choice, commit)  choice { option?, text? }; commit(ask,
--                                choice) after UNDO_MS unless undone
--   decide.undo(id), decide.pending_of(id)
--   decide.show(id), decide.hide(), decide.shown   the overlay's ask
--   decide.defer_times(now)      the Defer menu's choices: { key, at } (ms)

local data = require("@self/orch_data")

local M = {}

-- How long a pick can be taken back before it is sent.
M.UNDO_MS = 5000

export type Choice = { option: number?, text: string? }
-- (`sent`: past its Undo window, on its way.)
export type Pending = { option: number?, text: string?, label: string, sent: boolean? }

-- Picks not sent yet, by ask id.
M.pending = ui.state("orch_decide_pending", {} :: { [string]: Pending })
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
	local function nine(days_on: number): number
		local d = os.date("*t", t) :: any
		d.day += days_on
		d.hour, d.min, d.sec = 9, 0, 0
		return os.time(d)
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

-- A pick: shown as answered at once, sent after UNDO_MS unless undone.
-- One pick an ask; a second while one waits does nothing.
function M.pick(ask: any, choice: Choice, commit: (ask: any, choice: Choice) -> ())
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
	M.pending:set_key({ id }, { option = c.option, text = c.text, label = M.label_of(ask, c) })
	timers[id] = ui.after(M.UNDO_MS, function()
		timers[id] = nil
		local p = M.pending:peek()[id]
		if p then
			M.pending:set_key({ id }, { option = p.option, text = p.text, label = p.label, sent = true })
		end
		commit(ask, c)
		-- (Kept a beat past the send: the ask's answered state arrives and
		-- takes the row away, rather than it flashing back first.)
		ui.after(1500, function()
			M.pending:set_key({ id }, nil)
		end)
	end)
end

function M.undo(id: string)
	local cancel = timers[id]
	if cancel then
		cancel()
		timers[id] = nil
	end
	M.pending:set_key({ id }, nil)
end

function M.show(id: string)
	M.shown:set(id)
end

function M.hide()
	M.shown:set(nil)
end

return M

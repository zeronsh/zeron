--!strict
-- What agents ask without stopping for it, as the host keeps it
-- (src/questions.rs, the engine's `WatchQuestions`): each question on the
-- chat that asked it, on every device. An open question doesn't put its
-- chat in "waiting on you": the agent keeps working, and the engine sends
-- it the answer once one comes, as a message in its chat.
--
--   questions.now:get()       { state = "loading" | "ready" | "unsupported",
--                               list = { Question } }, oldest first
--   questions.of(chat)        that chat's questions shown: open ones and its
--                             last few settled ones (tracked)
--   questions.open()          every open one, oldest first (tracked)
--   questions.count(chat)     how many it has open (tracked)
--   questions.answer(id, text)  answered (an option's words, or your own)
--   questions.dismiss(id)     closed unanswered: the agent is told
--   questions.failed(id)      why the last answer to it didn't go (tracked)
--
-- An answer shows at once, settled; if the engine refuses it, the question
-- opens again with its reason.

local M = {}

export type Question = {
	id: string,
	chat: string,
	-- Markdown.
	question: string,
	options: { string },
	context: string?,
	status: "pending" | "answered" | "dismissed",
	answer: string?,
	-- Unix seconds.
	created: number,
	resolved: number?,
}
export type Now = { state: string, list: { Question } }

M.now = ui.shared("questions", { state = "loading", list = {} } :: Now)

-- Answers on their way: what was said (false: dismissed), by question.
local going = ui.state("questions_going", {} :: { [string]: string | boolean })
-- Why the last answer to a question didn't go, by question.
local refused = ui.state("questions_refused", {} :: { [string]: string })

local function set(state: any, id: string, value: any)
	local next = table.clone(state:peek())
	next[id] = value
	state:set(next)
end

-- The list as it reads with what's on its way applied.
local shown = ui.computed(function(): { Question }
	local list = M.now:get().list or {}
	local pending = going:get()
	if next(pending) == nil then
		return list
	end
	local out: { Question } = {}
	for _, q in list do
		local said = pending[q.id]
		if said ~= nil and q.status == "pending" then
			local settled = table.clone(q) :: any
			settled.status = if said == false then "dismissed" else "answered"
			settled.answer = if type(said) == "string" then said else nil
			table.insert(out, settled)
		else
			table.insert(out, q)
		end
	end
	return out
end)

function M.list(): { Question }
	return shown:get()
end

function M.of(chat: string?): { Question }
	local out: { Question } = {}
	if not chat then
		return out
	end
	for _, q in shown:get() do
		if q.chat == chat then
			table.insert(out, q)
		end
	end
	return out
end

function M.open(): { Question }
	local out: { Question } = {}
	for _, q in shown:get() do
		if q.status == "pending" then
			table.insert(out, q)
		end
	end
	return out
end

-- Open questions by chat, counted once per change of the list.
local counts = ui.computed(function(): { [string]: number }
	local out: { [string]: number } = {}
	for _, q in shown:get() do
		if q.status == "pending" then
			out[q.chat] = (out[q.chat] or 0) + 1
		end
	end
	return out
end)

function M.count(chat: string?): number
	return if chat then counts:get()[chat] or 0 else 0
end

function M.counts(): { [string]: number }
	return counts:get()
end

function M.failed(id: string): string?
	return refused:get()[id]
end

local function send(id: string, said: string | boolean, call: any)
	set(going, id, said)
	set(refused, id, nil)
	-- (Kept until the engine's own list settles it: below.)
	call:after(function() end, function(err: any)
		set(going, id, nil)
		set(refused, id, tostring(err))
	end)
end

-- What was on its way goes once the engine's list has settled it (or
-- has it no more).
ui.effect(function()
	local list = M.now:get().list or {}
	local pending = going:peek()
	if next(pending) == nil then
		return
	end
	local open: { [string]: boolean } = {}
	for _, q in list do
		if q.status == "pending" then
			open[q.id] = true
		end
	end
	local kept: { [string]: string | boolean } = {}
	local changed = false
	for id, said in pending do
		if open[id] then
			kept[id] = said
		else
			changed = true
		end
	end
	if changed then
		going:set(kept)
	end
end)

function M.answer(id: string, text: string)
	local said = string.match(text, "^%s*(.-)%s*$") or ""
	if said == "" then
		return
	end
	send(id, said, ui.call("questions.answer", { id = id, answer = said }))
end

function M.dismiss(id: string)
	send(id, false, ui.call("questions.dismiss", id))
end

return M

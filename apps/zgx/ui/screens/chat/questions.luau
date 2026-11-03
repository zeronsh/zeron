--!strict
-- What the open chat's agent asked without stopping for it
-- (@account/questions), pinned over the composer: the reply that asked
-- goes on streaming and scrolls away, while the question stays in reach
-- until it's answered. Open ones as cards (ds/question_card), oldest at
-- the top; under them, the chat's last few settled ones, each a line,
-- which × puts away (until another settles).
--
--   Questions.Stack(session)   session: () -> string?

local QuestionCard = require("@ds/question_card")
local IconButton = require("@ds/icon_button").IconButton
local questions = require("@account/questions")

local M = {}

-- How many settled ones show under the open ones.
local SETTLED = 3
-- Taller than this, the stack scrolls (the transcript keeps the rest).
local MAX_H = 360

-- Settled ones put away, by chat: those settled up to then.
local put_away = ui.state("chat_questions_put_away", {} :: { [string]: number })

function M.Stack(session: () -> string?): Node
	local mine = ui.computed(function(): { questions.Question }
		return questions.of(session())
	end)
	local open = ui.computed(function(): { questions.Question }
		local out: { questions.Question } = {}
		for _, q in mine:get() do
			if q.status == "pending" then
				table.insert(out, q)
			end
		end
		return out
	end)
	local settled = ui.computed(function(): { questions.Question }
		local id = session()
		local since = if id then put_away:get()[id] or 0 else 0
		local out: { questions.Question } = {}
		for _, q in mine:get() do
			if q.status ~= "pending" and (q.resolved or q.created) > since then
				table.insert(out, q)
			end
		end
		-- (The newest few.)
		while #out > SETTLED do
			table.remove(out, 1)
		end
		return out
	end)
	local function find(list: any, id: string, fallback: questions.Question): () -> questions.Question
		return function(): questions.Question
			for _, q in list:get() do
				if q.id == id then
					return q
				end
			end
			return fallback
		end
	end
	return ui.show(
		function(): boolean
			return #open:get() > 0 or #settled:get() > 0
		end,
		ui.scroll {
			id = "chat-questions",
			role = "group",
			label = ui.t("chat.questions.label") :: any,
			w_full = true,
			max_h = MAX_H,
			mb = "$space.2",
			overscroll = "contain",
			ui.column {
				w_full = true,
				gap = "$space.2",
				ui.each(open, function(q: questions.Question): Node
					local id = q.id
					return QuestionCard.Card {
						id = `chat-question-{id}`,
						question = find(open, id, q) :: any,
						on_answer = function(text: string)
							questions.answer(id, text)
						end,
						on_dismiss = function()
							questions.dismiss(id)
						end,
						failed = function(): string?
							return questions.failed(id)
						end,
					}
				end, {
					key = function(q: questions.Question): string
						return q.id
					end,
					w_full = true,
					gap = "$space.2",
					enter = { opacity = { 0, 1 }, y = { 6, 0 }, duration = 220, ease = "outCubic" },
					exit = { opacity = 0, duration = 140 },
				}),
				ui.show(
					function(): boolean
						return #settled:get() > 0
					end,
					ui.row {
						id = "chat-questions-settled",
						w_full = true,
						items = "start",
						ui.each(settled, function(q: questions.Question): Node
							return QuestionCard.Settled { id = `chat-question-{q.id}`, question = find(settled, q.id, q) :: any }
						end, {
							key = function(q: questions.Question): string
								return q.id
							end,
							grow = 1,
							min_w = 0,
						}),
						IconButton {
							icon = "close",
							label = ui.t("chat.questions.put_away") :: any,
							size = "sm",
							tone = "quiet",
							on_click = function()
								local id = session()
								if not id then
									return
								end
								local latest = 0
								for _, q in settled:peek() do
									latest = math.max(latest, q.resolved or q.created)
								end
								local next = table.clone(put_away:peek())
								next[id] = latest
								put_away:set(next)
							end,
						},
					}
				),
			},
		}
	)
end

return M

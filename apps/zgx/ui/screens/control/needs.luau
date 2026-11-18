-- Needs you, one for every place that shows it: the Control room's widget
-- (every orchestrator's) and an orchestrator's page (its own), the same
-- ds/orch_asks with the same answers, dismiss, recent answers and expand
-- into the decision overlay (ds/orch_decision), whose defer it carries.
--
--   needs.of(now, sessions_now, id?)    what waits on the person; with `id`,
--                                       only orchestrator `id`'s and its crew's
--   needs.Asks { now, sessions, orchestrator?, name_of, on_open?, recent?, limit? }
--   needs.Decision { name_of, hue_of, on_open?, on_link }   once per page
--   needs.open_chat(id, message?)       an orchestrator's page, else the session
--   needs.on_answer, needs.on_expand    a Board card's ask (ds/orch_board's
--                                       on_answer and on_expand_ask), as Needs you's

local ds = require("@ds")
local locale = require("@ds/locale")
local data = require("@ds/orch_data")
local Asks = require("@ds/orch_asks")
local Decision = require("@ds/orch_decision")
local decide = require("@ds/orch_decide")
local orchestration = require("@account/orchestration")
local route = require("@app/screens/shell/route")

local M = {}

-- A chat opened: an orchestrator's on its own page, any other as a session;
-- at `message` (an entry's id: the message an ask came from) when given.
function M.open_chat(id: string, message: string?)
	for _, o in orchestration.now:peek().orchestrators or {} do
		if o.id == id then
			route.orchestrator(id)
			return
		end
	end
	if message and message ~= "" then
		route.open_session(id, { message = message })
	else
		route.open_session(id)
	end
end

-- What waits on the person (data.needs); with `id`, only what orchestrator
-- `id` and its crew raised: its asks, and its own chat or a crew thread
-- waiting on an answer.
function M.of(now: any, sessions_now: any, id: string?): { any }
	local all = data.needs(now, sessions_now)
	if id == nil then
		return all
	end
	local crew = data.crew_by_chat(now)
	local out = {}
	-- (A chat waiting on its own ask is that ask, once: data.needs.)
	for _, n in all do
		local ask = n.ask
		if ask then
			if ask.orchestrator_id == id then
				table.insert(out, n)
			end
		elseif n.from == id or (crew[n.from or ""] and crew[n.from or ""].orchestrator_id == id) then
			table.insert(out, n)
		end
	end
	return out
end

-- The latest answers (decide.recent); with `id`, only to orchestrator
-- `id`'s asks (one answered here and not yet said by the engine included).
local function recent(now: any, id: string?): { any }
	local asks = now.asks or {}
	if id == nil then
		return decide.recent(asks, 5)
	end
	local mine: { [string]: boolean } = {}
	for _, a in asks do
		if a.orchestrator_id == id and type(a.id) == "string" then
			mine[a.id] = true
		end
	end
	local out = {}
	for _, r in decide.recent(asks, math.huge) do
		if mine[r.id] and #out < 5 then
			table.insert(out, r)
		end
	end
	return out
end

-- An ask answered (its option 1-based, or words, or both): from Needs you,
-- a card, or the overlay, past its Undo. A failure is said on the ask
-- (ds/orch_decide), with Retry.
function M.answer(ask: any, choice: any): any
	return orchestration.answer(ask.id, choice, ask)
end

-- A Board card waiting on a decision answers and opens as Needs you's rows
-- do: its chips answer it, its expand opens the whole of it in the overlay.
M.on_answer = M.answer
function M.on_expand(ask: any)
	decide.show(ask.id)
end

export type AsksProps = {
	-- The orchestration and sessions as the page reads them (held readers
	-- on the Control room).
	now: () -> any,
	sessions: () -> any,
	-- Only this orchestrator's (tracked); none for everyone's.
	orchestrator: (() -> string?)?,
	name_of: (id: string?) -> string?,
	on_open: ((chat: string, message: string?) -> ())?,
	-- false: no Recent answers at its foot (the floating Needs you's).
	recent: boolean?,
	limit: number?,
}

function M.Asks(props: AsksProps): Node
	local function of(): string?
		return if props.orchestrator then props.orchestrator() else nil
	end
	return Asks {
		needs = function(): { any }
			return M.of(props.now(), props.sessions(), of())
		end,
		recent = function(): { any }
			if props.recent == false then
				return {}
			end
			return recent(props.now(), of())
		end,
		name_of = props.name_of,
		on_answer = M.answer,
		on_dismiss = function(ask: any)
			orchestration.dismiss(ask.id):after(function() end, function(_err: any)
				ds.toast.show { title = locale.s("control.needs.failed"), tone = "danger" }
			end)
		end,
		on_open = props.on_open or M.open_chat,
		on_expand = function(ask: any)
			decide.show(ask.id)
		end,
		limit = props.limit,
	}
end

export type DecisionProps = {
	name_of: (id: string?) -> string?,
	hue_of: (id: string?) -> number?,
	on_open: ((chat: string, message: string?) -> ())?,
	on_link: (link: any) -> (),
}

-- The decision overlay: the ask Needs you or a card opened, whole; its
-- answer, defer, and the thread it came from.
function M.Decision(props: DecisionProps): Node
	return Decision {
		ask = function(): any
			local id = decide.shown:get()
			if not id then
				return nil
			end
			for _, a in orchestration.now:get().asks or {} do
				if a.id == id then
					return a
				end
			end
			return nil
		end,
		name_of = props.name_of,
		hue_of = props.hue_of,
		on_answer = M.answer,
		on_defer = function(ask: any, until_ms: number)
			decide.defer(ask, until_ms, function(a: any, ms: number): any
				return orchestration.defer(a.id, ms)
			end)
		end,
		on_open = props.on_open or M.open_chat,
		on_link = props.on_link,
	}
end

return M

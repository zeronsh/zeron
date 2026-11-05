-- Visits: each time a place opens (a setup step, a settings page), a new
-- visit. What a place asks Zeron as it opens (your agents, your profile,
-- the members) is asked once a visit, not again as a hot reload builds it
-- again: zgx tells a reload from a place opening again by nothing but kept
-- state like this.
--
--   local visits = require("@screens/visits").new("settings", function() return page end)
--   visits.once("members", load)        -- the first time this visit
--   visits.again("members", load)       -- asked again now (Try again)
--   visits.ask_until_answered("orgs", ask); visits.answer("orgs"); visits.answered("orgs")
--
-- `prefix` keeps each user's states apart; `place` says where it is (its
-- changing starts the next visit).

local M = {}

export type Visits = {
	visit: State<number>,
	once: (key: string, ask: () -> ()) -> (),
	again: (key: string, ask: () -> ()) -> (),
	ask_until_answered: (key: string, ask: () -> ()) -> (),
	answered: (key: string) -> boolean,
	answer: (key: string) -> (),
}

function M.new(prefix: string, place: () -> string?): Visits
	local visit = ui.state(`{prefix}_visit`, 0)
	local visited = ui.state(`{prefix}_visited`, "")
	local asked = ui.state(`{prefix}_asked_in_visit`, {} :: { [string]: number })
	local answered = ui.state(`{prefix}_answered_in_visit`, {} :: { [string]: number })
	-- (A reload asks again only while an answer is still to come, since it
	-- would come to the scripts that asked.)
	local asking_here: { [string]: number } = {}

	-- (The visit counts on as the place changes, whichever reads it first:
	-- the place being built, or this effect.)
	local function this_visit(): number
		local here = place() or ""
		if here ~= visited:peek() then
			visited:set(here)
			visit:set(visit:peek() + 1)
		end
		return visit:peek()
	end
	ui.effect(function()
		place()
		this_visit()
	end)

	local v = {} :: Visits
	v.visit = visit
	function v.once(key: string, ask: () -> ())
		local now = this_visit()
		if asked:peek()[key] ~= now then
			asked:set_key(key, now)
			ask()
		end
	end
	function v.again(key: string, ask: () -> ())
		asked:set_key(key, this_visit())
		ask()
	end
	function v.ask_until_answered(key: string, ask: () -> ())
		local now = this_visit()
		if answered:peek()[key] == now or asking_here[key] == now then
			return
		end
		asking_here[key] = now
		ask()
	end
	-- `key`'s answer came in this visit (tracked).
	function v.answered(key: string): boolean
		return answered:get()[key] == visit:get()
	end
	function v.answer(key: string)
		answered:set_key(key, visit:peek())
	end
	return v
end

return M

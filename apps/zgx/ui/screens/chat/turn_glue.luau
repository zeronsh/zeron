-- A transcript's `turn_state` for a session (screens/chat/turn_state, the
-- rules): what its host is (its Cloud session, the device it's on) read here,
-- the rules over there.
--
--   T.Transcript { turn_state = TurnGlue.of(function() return store.session:get() end), … }

local cloud = require("@account/cloud")
local sessions = require("@account/sessions")
local TurnState = require("@app/screens/chat/turn_state")

local M = {}

-- Moved once a just-begun reply's grace is over, so it's judged again then;
-- nothing ticks otherwise.
local TICK = ui.state("chat_turn_tick", 0)
local due: { [number]: boolean } = {}

local function judge_again_at(at: number)
	local key = math.ceil(at)
	if due[key] then
		return
	end
	due[key] = true
	ui.after(math.max(0, (key - os.time()) * 1000) + 100, function()
		due[key] = nil
		TICK:set(TICK:peek() + 1)
	end)
end

function M.of(session: () -> any): (m: any, sending: boolean) -> TurnState.State?
	return function(m: any, sending: boolean): TurnState.State?
		local s = session()
		if not s or s.state ~= "ready" then
			return nil
		end
		TICK:get()
		local now = os.time()
		local started = m.started or m.since
		if started and now - started < TurnState.GRACE then
			judge_again_at(started + TurnState.GRACE)
		end
		local machine = if cloud.on_device(s.device) then cloud.session_for(s.id, s.device) else nil
		local unreachable, host = false, nil
		local here = sessions.now:get().here
		if not machine and s.device and s.device ~= here then
			for _, d in sessions.now:get().devices or {} do
				if d.id == s.device then
					unreachable, host = d.online == false, d.name
				end
			end
		end
		return TurnState.of {
			streaming = true,
			sending = sending,
			live = s.running == true,
			started = started,
			now = now,
			cloud = machine,
			host_unreachable = unreachable,
			host = host,
		}
	end
end

return M

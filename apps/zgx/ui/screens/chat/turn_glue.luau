-- A transcript's `turn_state` for a session (screens/chat/turn_state, the
-- rules): what its host is (its Cloud session, the device it's on) read here,
-- the rules over there.
--
--   T.Transcript { turn_state = TurnGlue.of(function() return store.session:get() end, store.messages), … }

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

-- Whether anything in the chat's transcript is a reply.
local function answered(messages: any): boolean
	for i = messages:len(), 1, -1 do
		if messages:at(i).role == "assistant" then
			return true
		end
	end
	return false
end

-- (`messages`: the chat's transcript, to tell a new chat on Cloud.)
function M.of(session: () -> any, messages: any?): (m: any, sending: boolean) -> TurnState.State?
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
		-- (A new chat on Cloud, before Cloud has made it: its machine to come.)
		local on_cloud = cloud.on_device(s.device) or s.cloud == true
		local machine: any = if on_cloud then cloud.session_for(s.id, s.device) else nil
		-- On its way to a Cloud machine: what the machine says of itself is
		-- read closely a while (a woken one may update before it answers).
		if sending and on_cloud and m.id then
			cloud.sending(m.id)
		end
		-- On Cloud, its machine not known yet, in a chat nothing has answered
		-- (a new chat's, before the sessions are read again): its first send
		-- starts one.
		if sending and on_cloud and not machine and messages and not answered(messages) then
			machine = { state = "provisioning", unknown = true }
		end
		local wake_failed = false
		if sending and machine and machine.chat then
			wake_failed = cloud.wake_state(machine.chat).failure ~= nil
		end
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
			wake_failed = wake_failed,
			held = m.held == true,
			setup = type(m.id) == "string" and string.sub(m.id, -12) == ".cloud-setup",
			host_unreachable = unreachable,
			host = host,
		}
	end
end

return M

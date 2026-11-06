-- A chat's Cloud machine, on its composer's deck: tracked reads and the
-- actions, nothing drawn. The composer takes its state as a notice and its one
-- alert (screens/chat/cloud_machine) with the sizes under it as an alert of
-- the deck's, in ds/composer_dock's own rows.
--
--   local machine = CloudTray.bind { chat = fn, device = fn, running = fn }
--   Composer { alerts = machine.alerts, on_alert_action = machine.on_alert,
--     on_dismiss_alert = machine.dismiss, on_notice_action = machine.wake, … }
--   LinkNotice.of { cloud = machine.cloud_link(), … }

local cloud = require("@account/cloud")
local logic = require("@app/screens/chat/cloud_machine")
local route = require("@app/screens/shell/route")

local M = {}

-- What's been put away, by chat (logic.dismiss): kept across launches.
local PUT_AWAY = ui.persist("cloud_alerts_put_away", {} :: { [string]: { [string]: string } })
-- The hour, for what's said of a date: enough to move "in 3 days" on its own,
-- and nothing draws faster for it.
local HOUR = ui.state("cloud_alerts_hour", 0)

export type Bind = {
	chat: () -> string?,
	device: () -> string?,
	running: (() -> boolean)?,
}

function M.bind(c: Bind): any
	local b: any = {}
	-- Is this chat's machine one of Cloud's? Then its sessions are followed.
	cloud.follow(function(): boolean
		return c.chat() ~= nil and cloud.on_device(c.device())
	end)
	ui.effect(function()
		if not cloud.on_device(c.device()) then
			return
		end
		HOUR:set(os.time() // 3600)
		ui.every(60000, function()
			local h = os.time() // 3600
			if HOUR:peek() ~= h then
				HOUR:set(h)
			end
		end)
	end)
	function b.session(): any
		return cloud.session_for(c.chat(), c.device())
	end
	-- Its levels fell or cleared: what was put away moves with them.
	ui.effect(function()
		local s = b.session()
		-- (Not before the hour's known: every date would read as far off.)
		if s and HOUR:get() > 0 then
			local settled = logic.settle(PUT_AWAY:peek(), s, HOUR:get() * 3600)
			if settled ~= PUT_AWAY:peek() then
				PUT_AWAY:set(settled)
			end
		end
	end)
	-- A session's chat: a side chat acts on its parent's machine.
	local function owner(): string?
		local s = b.session()
		return s and s.chat
	end
	function b.wake_state(): any
		return cloud.wake_state(owner())
	end
	b.alert = ui.computed(function(): logic.Alert?
		local s = b.session()
		if not s then
			return nil
		end
		local put_away = PUT_AWAY:get()
		return logic.alert(s, {
			now = HOUR:get() * 3600,
			failure = b.wake_state().failure,
			dismissed = function(kind: string, level: string): boolean
				return logic.dismissed(put_away, s.chat, kind, level)
			end,
		})
	end)
	b.picker = ui.computed(function(): logic.Picker?
		local s, a = b.session(), b.alert:get()
		if not s or not a or not a.picks then
			return nil
		end
		return logic.picker(s, cloud.machines(), a, { running = c.running ~= nil and c.running() })
	end)
	-- The deck's alerts: the one alert, its sizes under it.
	b.alerts = ui.computed(function(): { any }
		local a = b.alert:get()
		if not a then
			return {}
		end
		local w = b.wake_state()
		return { logic.deck_alert(a, b.picker:get(), { failure = w.failure, busy = w.busy }) }
	end)
	-- For LinkNotice.of: whether this chat is one of Cloud's, and its line.
	-- (Files going says it's asleep already: not two alerts for one machine.)
	function b.cloud_link(): any
		local s = b.session()
		if not s then
			return nil
		end
		local a = b.alert:get()
		return { line = if a and a.kind == "expiring" then nil else logic.line(s) }
	end
	function b.dismiss()
		local s, a = b.session(), b.alert:peek()
		if not s or not a then
			return
		end
		if a.kind == "wake_failed" then
			cloud.clear_failure(s.chat)
		elseif a.level then
			PUT_AWAY:set(logic.dismiss(PUT_AWAY:peek(), s.chat, a.kind, a.level))
		end
	end
	-- The actions: a size picked, the line's Wake, and the alert's way on.
	function b.pick(machine: string)
		local s = b.session()
		if not s then
			return
		end
		if s.state == "ready" then
			cloud.resize(s.chat, machine)
		else
			cloud.wake(s.chat, machine)
		end
	end
	function b.wake()
		local chat = owner()
		if chat then
			cloud.wake(chat)
		end
	end
	function b.act(action: string?)
		local s = b.session()
		if not s then
			return
		end
		-- (The chat's machine: a pick of a size, or the alert's way on.)
		if action == "fresh" then
			cloud.fresh(s.chat, cloud.machine())
		elseif action == "retry" or action == "wake" then
			cloud.wake(s.chat)
		elseif action == "new_chat" then
			route.new_chat(s.space)
		end
	end
	-- What the deck's told: a size's row, or the alert's own action.
	function b.on_alert(id: string)
		local size = logic.size_of(id)
		if size then
			b.pick(size)
			return
		end
		local a = b.alert:peek()
		if a then
			local action = logic.way_on(a, b.picker:peek())
			b.act(action)
		end
	end
	return b
end

return M

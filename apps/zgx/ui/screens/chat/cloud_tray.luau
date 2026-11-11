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
local Held = require("@app/screens/chat/held")

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
	-- The chat's own queue (each `{ at? }`, seconds), what its machine sends as it
	-- wakes: one that waited long is said before it goes.
	queue: (() -> { any })?,
	-- Its messages on their way and not yet taken (the session's `waiting`,
	-- none from an older engine), and sending one again (its message id).
	waiting: (() -> { any }?)?,
	retry: ((message: string?) -> ())?,
	-- Whether the chat's newest message is a prompt on its way: its machine
	-- coming up is said under it in the transcript then, not in the deck.
	sending: (() -> boolean)?,
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
	-- Why a send to this chat is kept back now, if it is.
	function b.hold_reason(): string?
		return logic.hold_reason(b.session())
	end
	function b.now(): number
		-- (Read again as the hour turns: enough for "3 days ago", and nothing
		-- ticks faster for it.)
		HOUR:get()
		return os.time()
	end
	-- What has waited past LONG_WAIT: how many, and the oldest's age.
	local function waited(list: { any }, at: (any) -> number?): logic.Waiting?
		local now, count, oldest = b.now(), 0, 0
		for _, x in list do
			local t = at(x)
			if t and now - t > logic.LONG_WAIT then
				count += 1
				oldest = math.max(oldest, now - t)
			end
		end
		return if count > 0 then { count = count, age = oldest } else nil
	end
	local sent_failed: () -> { failure: string, message: string? }?
	-- What its machine runs as it wakes: its queue, and what's on its way to it
	-- not yet taken (those that won't run any more left out).
	local function about_to_go(): { any }
		local out = {}
		for _, q in (if c.queue then c.queue() else {}) do
			table.insert(out, q)
		end
		local waiting = logic.waiting_of(if c.waiting then c.waiting() else nil, b.now())
		for _, w in waiting do
			table.insert(out, w)
		end
		return out
	end
	sent_failed = function(): { failure: string, message: string? }?
		local _, failed = logic.waiting_of(if c.waiting then c.waiting() else nil, b.now())
		return failed
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
			send_failed = sent_failed(),
			queued_old = waited(about_to_go(), function(q: any): number?
				return q.at
			end),
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
		local s = b.session()
		return {
			logic.deck_alert(a, b.picker:get(), {
				failure = w.failure,
				busy = w.busy,
				held = if s then #Held.of(s.chat) else 0,
				reason = logic.hold_reason(s),
			}),
		}
	end)
	-- For LinkNotice.of: whether this chat is one of Cloud's, and its line.
	-- (Files going says it's asleep already: not two alerts for one machine.)
	-- `intent`: something to send (a draft, a queue): asleep is said then.
	function b.cloud_link(intent: boolean?): any
		local s = b.session()
		if not s then
			return nil
		end
		local a = b.alert:get()
		local line = if a and a.kind == "expiring" then nil else logic.line(s, { intent = intent })
		if line and line.boot and c.sending and c.sending() then
			line = nil
		end
		return { line = line }
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
		elseif action == "retry_send" and c.retry then
			local failed = sent_failed()
			c.retry(if failed then failed.message else nil)
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

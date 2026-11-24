--!strict
-- What a chat's subagents ask the person, on the chat that delegated to them:
-- a child chat isn't in the sidebar, and may run on another device, so an
-- approval it needs shows over its parent's composer (the deck's alerts,
-- ds/composer_dock) and is answered there.
--
--   local d = Delegates.bind { chat = fn, alerts = state, on_alert = fn, failed = fn, asked = fn }
--   ds.Composer { alerts = d.alerts, on_alert_action = d.on_alert, … }
--
-- `alerts` and `on_alert`: the composer's others, which come after. A
-- question isn't listed: the parent answers those it can, and its own lane
-- and the tray's row show it meanwhile.

local locale = require("@ds/locale")
local sessions = require("@account/sessions")
local lanes = require("@app/screens/chat/lanes")
local store = require("@app/screens/chat/store")

local M = {}

local ASK, ALLOW, DENY, OPEN = "ask:", "allow:", "deny:", "open:"
local asks = ui.shared("chat.asks", {} :: { [string]: string })
local details = ui.shared("chat.ask_details", {} :: { [string]: any })

export type Props = {
	chat: () -> string?,
	alerts: State<{ any }>,
	on_alert: (id: string) -> (),
	-- An answer that didn't reach it.
	failed: () -> (),
	-- Something new is asked: shown open, with nothing else open there.
	asked: () -> (),
}

-- `id` without `prefix`, nil where it doesn't start with it.
local function after(id: string, prefix: string): string?
	return if string.sub(id, 1, #prefix) == prefix then string.sub(id, #prefix + 1) else nil
end

local function device_name(id: string): string?
	for _, d in sessions.now:get().devices or {} do
		if d.id == id then
			return d.name
		end
	end
	return nil
end

function M.bind(props: Props): { alerts: State<{ any }>, on_alert: (id: string) -> () }
	local alerts = ui.computed(function(): { any }
		local out: { any } = {}
		local asked = asks:get()
		if next(asked) ~= nil then
			local told = details:get()
			for _, c in sessions.children(props.chat()) do
				local ask = told[c.id]
				if c.child_kind ~= nil and asked[c.id] == "approval" and ask and ask.kind == "approval" then
					local said: { string } = {}
					for _, word in { c.harness_name or "", device_name(c.device) or "" } do
						if word ~= "" then
							table.insert(said, word)
						end
					end
					local function choice(prefix: string, key: string): any
						return { id = `{prefix}{c.id}`, tone = "warning", title = "", action = locale.s(key) }
					end
					table.insert(out, {
						id = `{ASK}{c.id}`,
						tone = "warning",
						title = locale.s(if ask.command then "chat.delegates.asks" else "chat.delegates.asks_approval", { title = c.title }),
						detail = table.concat(said, " · "),
						body = ask.command or ask.detail,
						dismissable = false,
						buttons = {
							choice(DENY, "chat.delegates.deny"),
							choice(ALLOW, "chat.delegates.allow"),
							choice(OPEN, "chat.delegates.open"),
						},
					})
				end
			end
		end
		for _, a in props.alerts:get() do
			table.insert(out, a)
		end
		return out
	end)
	-- Each request is put in front of the person once: folded again, it stays
	-- so.
	local shown: State<{ [string]: boolean }> = ui.state("chat_delegate_asks_shown", {} :: { [string]: boolean })
	ui.effect(function()
		local told = details:get()
		local fresh = false
		local now = table.clone(shown:peek())
		for _, a in alerts:get() do
			local id = after(a.id, ASK)
			local request = id and told[id] and told[id].request
			if request and not now[request] then
				now[request] = true
				fresh = true
			end
		end
		if fresh then
			shown:set(now)
			props.asked()
		end
	end)
	local function answer(id: string, decision: string)
		local ask = details:peek()[id]
		store.respond_child(id, if ask then ask.request else "", decision):after(function() end, props.failed)
	end
	return {
		alerts = alerts,
		on_alert = function(id: string)
			local allowed, denied, opened = after(id, ALLOW), after(id, DENY), after(id, OPEN)
			local parent = props.chat()
			if allowed then
				answer(allowed, "once")
			elseif denied then
				answer(denied, "deny")
			elseif opened and parent then
				for _, c in sessions.children(parent) do
					if c.id == opened then
						lanes.open(parent, { kind = "side_chat", id = c.id, title = c.title })
					end
				end
			else
				props.on_alert(id)
			end
		end,
	}
end

return M

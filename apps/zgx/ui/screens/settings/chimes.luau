-- The sessions' chimes, as the shipping app plays them (crates/ui
-- shell.rs, its one session detector): a chat turning done, waiting on you,
-- or failed plays its sound, if Settings → Notifications has it on. A
-- chat's first appearance only sets where it stands (a launch, a sync, a
-- hot reload never ring), and what it was is kept in state, so a reload
-- picks up where it was.
--
-- And the window's focus, told to the engine (`SetViewportFocus`), which
-- holds its banners while Zeron is the app in front if Notifications says
-- "Only when in the background". Told when it changes, then again every 20
-- seconds while it holds (the engine forgets a report that goes stale).

local host = require("@app/screens/settings/host")
local sessions = require("@account/sessions")
local prefs = require("@app/screens/settings/prefs")

local M = {}

-- Each chat's status as last seen, by id ("" for idle).
local seen = ui.state("settings_chimes_seen", {} :: { [string]: string })

-- What a status turning on plays.
local SOUND: { [string]: string } = {
	done = "done",
	input = "request",
	failed = "attention",
}

ui.effect(function()
	local now = sessions.now:get()
	if now.state ~= "ready" then
		return
	end
	local before = seen:peek()
	local after: { [string]: string } = {}
	local ring: string? = nil
	for _, c in now.chats do
		local status = c.status or ""
		after[c.id] = status
		local was = before[c.id]
		if was ~= nil and was ~= status and SOUND[status] then
			-- (One chime for many at once: the most urgent.)
			local sound = SOUND[status]
			if ring == nil or sound == "attention" or (sound == "request" and ring == "done") then
				ring = sound
			end
		end
	end
	seen:set(after)
	if ring and prefs.chimes(ring) then
		host.invoke("sound.play", ring)
	end
end)

-- The window's focus, as last told (nil: not yet).
local VIEWPORT = "zgx-main"
local told = ui.state("settings_focus_told", nil :: boolean?)
local function tell(focused: boolean)
	told:set(focused)
	host.call("settings.engine", { method = "SetViewportFocus", params = { viewportId = VIEWPORT, focused = focused } })
		:after(function() end, function() end)
end
ui.effect(function()
	local active = ui.window.active:get()
	if active ~= told:peek() then
		tell(active)
	end
	-- Focused, said again every 20 s: the engine lets a report go stale
	-- (crates/engine/src/desktop_notify.rs, `TTL`). Only while focused (a
	-- timer made in an effect stops as it runs again), so an unfocused
	-- window has no timer at all.
	if active then
		ui.every(20000, function()
			tell(true)
			return true
		end)
	end
end)

return M

-- Appshots, as the shipping app has them (crates/ui appshots.rs): the
-- window in front of another app, captured on a global shortcut by the host
-- (src/appshots: off the window's thread), lands on a composer as a chip.
-- It is never sent on its own; sent, its picture goes as an attachment and
-- the app's name, window title and (with Accessibility) its text go after
-- the words as untrusted observed content (the host writes that block).
--
-- Where a capture lands (Settings → Appshots → Destination):
-- - Automatic: the open session's composer, or a new chat's when none is
--   open;
-- - Last session: the open session, or back to the last one an Appshot
--   went to;
-- - New session: a new chat's composer, the drafts there are kept.
-- What's staged is held to 96 MB of Appshots, as the shipping app's.
--
-- The settings are this Mac's (kept on disk) and handed to the host at
-- launch: whether Appshots are on, the sound, the shortcut.

local route = require("@app/screens/shell/route")
local shared = require("@app/screens/chat/shared")
local toast = require("@ds/toast")

local M = {}

M.state = ui.shared("appshots.state", nil :: any)
local captured = ui.shared("appshots.captured", nil :: any)
local failed = ui.shared("appshots.failed", nil :: any)

M.enabled = ui.persist("appshots_enabled", false)
M.sound = ui.persist("appshots_sound", true)
M.shortcut = ui.persist("appshots_shortcut", "ctrl+alt+space")
M.destination = ui.persist("appshots_destination", "automatic")
-- The last session an Appshot went to (Last session's).
local last_session = ui.persist("appshots_last_session", "")
-- The capture last taken in (kept: a reload takes none twice).
local taken = ui.state("appshots_taken", 0)

-- The staged Appshots' limit (the shipping app's MAX_STAGED_APPSHOT_BYTES).
M.MAX_STAGED = 96 * 1024 * 1024

local function invoke(name: string, arg: any?)
	pcall(ui.invoke, name, arg)
end

-- The settings, to the host (as they change, and at launch).
ui.effect(function()
	invoke("appshots.configure", { enabled = M.enabled:get(), sound = M.sound:get(), shortcut = M.shortcut:get() })
end)

function M.set_recording(on: boolean)
	invoke("appshots.configure", { recording = on })
end
function M.refresh()
	invoke("appshots.refresh")
end
-- Asks for a permission ("capture" or "text"); asked once already, opens
-- System Settings at it.
function M.request(which: string)
	invoke("appshots.request", { which = which })
end

-- A capture as the composer carries it.
function M.chip(shot: any, id: string): any
	return {
		id = id,
		kind = "appshot",
		name = if shot.title and shot.title ~= "" then shot.title else shot.app,
		meta = `{shot.app} · Appshot`,
		src = shot.path,
		path = shot.path,
		icon = shot.icon,
		bytes = shot.bytes,
		appshot = {
			app = shot.app,
			bundle = shot.bundle,
			title = shot.title,
			text = shot.text,
			truncated = shot.truncated == true,
		},
	}
end

-- What a send carries of its Appshots: one each, its picture's path here
-- and what the host writes of it.
function M.of_cargo(items: { any }): { any }
	local out = {}
	for _, item in items do
		if item.kind == "appshot" and item.path and item.appshot then
			local a = item.appshot
			table.insert(out, { path = item.path, app = a.app, bundle = a.bundle, title = a.title, text = a.text, truncated = a.truncated })
		end
	end
	return out
end

-- How many bytes of Appshots are staged.
local function staged(): number
	local n = 0
	for _, item in shared.cargo:peek() do
		if item.kind == "appshot" then
			n += item.bytes or 0
		end
	end
	return n
end

local chips = 0
-- A capture landing: to its destination's composer, unless that would pass
-- the limit.
function M.land(shot: any)
	if staged() + (shot.bytes or 0) > M.MAX_STAGED then
		toast.show { title = ui.t("chat.appshots.limit"):get(), tone = "warning" }
		return
	end
	local now = route.current:peek()
	local where = M.destination:peek()
	if where == "new" then
		route.new_chat()
	elseif where == "last" and now.name ~= "session" and last_session:peek() ~= "" then
		route.open_session(last_session:peek())
	elseif now.name ~= "session" and now.name ~= "new" then
		route.new_chat()
	end
	local at = route.current:peek()
	if at.name == "session" and at.session then
		last_session:set(at.session)
	end
	chips += 1
	shared.cargo:push(M.chip(shot, `appshot{chips}-{shot.id}`))
	shared.focus:add(1)
end

-- Each capture the host takes, once.
ui.effect(function()
	local c = captured:get()
	if type(c) ~= "table" or not c.n or c.n <= taken:peek() then
		return
	end
	taken:set(c.n)
	M.land(c.shot)
end)
-- Each that couldn't be taken, said once.
local said = ui.state("appshots_said", 0)
ui.effect(function()
	local f = failed:get()
	if type(f) ~= "table" or not f.n or f.n <= said:peek() then
		return
	end
	said:set(f.n)
	toast.show { title = ui.t("chat.appshots.couldnt_capture"):get(), body = f.message, tone = "danger" }
end)

return M

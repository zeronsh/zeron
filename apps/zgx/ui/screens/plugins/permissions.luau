--!strict
-- The macOS permissions a plugin's settings page can show a row for (a
-- `permission` setting, Zeron's own plugins alone): Zeron's words for each,
-- whether it's granted, and the way to grant it. The host already checks
-- both for Appshots (src/appshots: `appshots.state`'s `capture` and `text`,
-- "ready" or "required"; `appshots.request` asks once, then opens System
-- Settings at it; `appshots.refresh` reads them again), so these rows read
-- the same. A fixture replaces `request` and `refresh`.

local M = {}

export type Permission = {
	title: string,
	-- What it lets the plugin do, its name said (`name`).
	description: (name: string) -> string,
	-- The button's first words, before System Settings.
	grant: string,
	-- Its key in `appshots.state`, and in `appshots.request`.
	key: string,
}

-- (Its words read in the locale as they're asked for: `title` and `grant`
-- through their messages, each time.)
local function known(words: string, key: string): Permission
	return setmetatable({
		description = function(name: string): string
			return ui.t(`{words}.description`, { name = name }):get()
		end,
		key = key,
	}, {
		__index = function(_, field: string): string?
			if field == "title" or field == "grant" then
				return ui.t(`{words}.{field}`):get()
			end
			return nil
		end,
	}) :: any
end

M.KNOWN = {
	-- plugins.permissions.screen_recording.{title, description, grant}
	["screen-recording"] = known("plugins.permissions.screen_recording", "capture"),
	-- plugins.permissions.accessibility.{title, description, grant}
	accessibility = known("plugins.permissions.accessibility", "text"),
} :: { [string]: Permission }

-- Said under a section with permission rows (a message, made as it's asked for).
function M.note(): Message
	return ui.t("plugins.permissions.note")
end

M.state = ui.shared("appshots.state", nil :: any)

-- Whether it's granted now (followed).
function M.granted(permission: string): boolean
	local p = M.KNOWN[permission]
	local now = M.state:get()
	return p ~= nil and type(now) == "table" and now[p.key] == "ready"
end

-- Asks for it: macOS's own prompt the first time, then System Settings at it.
function M.request(permission: string)
	local p = M.KNOWN[permission]
	if p then
		pcall(ui.invoke, "appshots.request", { which = p.key })
	end
end

-- Reads them all again (after a trip to System Settings).
function M.refresh()
	pcall(ui.invoke, "appshots.refresh", nil)
end

return M

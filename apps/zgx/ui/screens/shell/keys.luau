-- The main window's shortcuts, one list: what the shell's commands are
-- made from (screens/shell/init, each its `ui.command`, its menu item and
-- its key) and what Settings → Shortcuts lists. Zeron's defaults (crates/ui
-- settings.rs `default_combo_on`), as zgx's actions have them.
--
--   keys.LIST            { { id, title, key, group, menu, reserved? }, … }
--                        in the order they're shown (and made)
--   keys.get(id)         one, by its command's id
--   keys.combo(id)       its combination now, a rebind's or its default
--                        (keys.key(id): the same, for `ui.command`'s key)
--   keys.bind / reset / reset_all / owner   Settings → Shortcuts' rebinding
--
-- `group`: where Settings lists it ("Sessions", "Panels", "Go", "App");
-- `menu`: the menu bar's menu it's in; `reserved`: a focused terminal
-- never has it first (Zeron's keep their shortcuts from a page);
-- `elsewhere`: another screen makes its command (the shell makes the rest).

export type Key = {
	id: string,
	title: string,
	key: string,
	group: string,
	menu: string,
	reserved: boolean?,
	-- Made by its own screen, not the shell (its title and key kept the
	-- same there).
	elsewhere: boolean?,
}

local M = {}

M.LIST = {
	{ id = "shell.new_chat", title = "New chat", key = "cmd+n", group = "Sessions", menu = "File", reserved = true },
	{ id = "shell.new_project", title = "New project", key = "cmd+shift+n", group = "Sessions", menu = "File", reserved = true },
	{ id = "shell.archive", title = "Archive session", key = "cmd+shift+a", group = "Sessions", menu = "File", reserved = true },
	{ id = "shell.pin", title = "Pin or unpin this session", key = "cmd+shift+p", group = "Sessions", menu = "File", reserved = true },
	{ id = "shell.model_picker", title = "Choose model", key = "cmd+/", group = "Sessions", menu = "View", reserved = true },
	{ id = "shell.toggle_sidebar", title = "Toggle sidebar", key = "cmd+b", group = "Panels", menu = "View", reserved = true },
	{ id = "shell.toggle_files", title = "Toggle panel", key = "cmd+r", group = "Panels", menu = "View", reserved = true },
	{ id = "shell.toggle_files_tab", title = "Toggle files", key = "cmd+e", group = "Panels", menu = "View", reserved = true },
	{ id = "shell.toggle_terminal", title = "Toggle terminal", key = "cmd+j", group = "Panels", menu = "View", reserved = true },
	-- (The browser tab's own command, screens/panel/browser: listed here so
	-- Settings shows it and no other takes its key; it acts while a browser
	-- tab shows.)
	{ id = "panel.annotate", title = "Annotate Element", key = "cmd+shift+i", group = "Panels", menu = "View", elsewhere = true },
	{ id = "panel.new_browser_tab", title = "New Browser Tab", key = "cmd+t", group = "Panels", menu = "File", elsewhere = true },
	{ id = "panel.focus_address", title = "Open Location", key = "cmd+l", group = "Panels", menu = "View", elsewhere = true },
	{ id = "panel.reload_page", title = "Reload Page", key = "cmd+shift+r", group = "Panels", menu = "View", elsewhere = true },
	{ id = "shell.next_session", title = "Next session", key = "ctrl+tab", group = "Go", menu = "Go", reserved = true },
	{ id = "shell.previous_session", title = "Previous session", key = "ctrl+shift+tab", group = "Go", menu = "Go", reserved = true },
} :: { Key }
-- ⌘1–9: the sidebar's first nine sessions.
for i = 1, 9 do
	table.insert(M.LIST, { id = `shell.session_{i}`, title = `Session {i}`, key = `cmd+{i}`, group = "Go", menu = "Go", reserved = true })
end
for _, k in
	{
		{ id = "shell.palette", title = "Search", key = "cmd+k", group = "App", menu = "File", reserved = true },
		{ id = "shell.settings", title = "Settings", key = "cmd+,", group = "App", menu = "File", reserved = true },
		{ id = "shell.find", title = "Find", key = "cmd+f", group = "App", menu = "Edit" },
		{ id = "shell.close", title = "Close", key = "cmd+w", group = "App", menu = "File", reserved = true },
		-- (No key of their own: Settings → Shortcuts can give them one.)
		{ id = "shell.reload", title = "Reload Zeron", key = "", group = "App", menu = "View" },
		{ id = "shell.reload_interface", title = "Reload interface", key = "", group = "App", menu = "View" },
	} :: { Key }
do
	table.insert(M.LIST, k)
end

local by_id: { [string]: Key } = {}
for _, k in M.LIST do
	by_id[k.id] = k
end
function M.get(id: string): Key?
	return by_id[id]
end

-- What Settings → Shortcuts has rebound, by id: a combination in place of
-- the default ("" for none). Each command's `key` follows it (`M.key`), so a
-- rebind takes effect at once, menus and all, with no reload.
-- (None where there's no interface: a test reading the table alone.)
M.bound = (if ui ~= nil then ui.persist("shell_keymap", {} :: { [string]: string }) else nil) :: State<{ [string]: string }>

-- The combination `id` has now ("" for none), tracked: its own, else its
-- default.
function M.combo(id: string): string
	local own = if M.bound then M.bound:get()[id] else nil
	if own ~= nil then
		return own
	end
	local k = by_id[id]
	return if k then k.key else ""
end
-- A command's `key`: its combination, following a rebind.
function M.key(id: string): () -> string
	return function(): string
		return M.combo(id)
	end
end
-- Whether `id` has a combination of its own (Reset shows), tracked.
function M.customized(id: string): boolean
	return M.bound:get()[id] ~= nil
end
-- `id` on `combo` ("" for none); its default again drops the binding.
function M.bind(id: string, combo: string)
	local map = table.clone(M.bound:peek())
	local k = by_id[id]
	map[id] = if k and k.key == combo then nil else combo
	M.bound:set(map)
end
function M.reset(id: string)
	if M.bound:peek()[id] ~= nil then
		M.bind(id, (by_id[id] :: Key).key)
	end
end
function M.reset_all()
	if next(M.bound:peek()) ~= nil then
		M.bound:set({})
	end
end
-- The command (its id: the app's, or a plugin's `<plugin>/<id>`) holding
-- `combo` now, other than `id`, or nil.
function M.owner(combo: string, id: string?): string?
	local owner = ui.shortcut_owner(combo)
	return if owner ~= id then owner else nil
end

-- What a shortcut is called in the locale shown (`title` stays the English
-- it's kept and compared by): read where it's drawn, or once as its
-- command is made.
function M.title(k: Key): string
	local n = string.match(k.id, "^shell%.session_(%d+)$")
	if n then
		return ui.t("shell.keys.title.session_n", { i = n }):get()
	end
	local key = `shell.keys.title.{(string.gsub(k.id, "%.", "_"))}`
	local said = ui.t(key):get()
	return if said == key then k.title else said
end

-- A group's name in the locale shown (`group` stays the English).
local GROUPS: { [string]: string } = {
	Sessions = "shell.keys.group.sessions",
	Panels = "shell.keys.group.panels",
	Go = "shell.keys.group.go",
	App = "shell.keys.group.app",
}
function M.group_title(group: string): string
	local key = GROUPS[group]
	return if key then ui.t(key):get() else group
end

return M

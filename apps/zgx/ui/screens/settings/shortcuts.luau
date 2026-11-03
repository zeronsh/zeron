-- Settings → Shortcuts: every shortcut Zeron has, grouped as the shipping
-- app's page groups them, each with its keys. The list is the shell's own
-- table (screens/shell/keys `LIST`: `{ id, title, key, group, menu }` in
-- display order), which its `ui.command`s are declared from, so the page can't
-- drift from what the keys do. Each shortcut is searchable.
--
-- Rebinding, as the shipping app's page (crates/ui settings/shortcuts.rs):
-- click a combination and press a new one (Escape cancels); one another
-- command holds is refused, the page saying whose it is (`ui.shortcut_owner`,
-- through keys.owner); Reset on a row rebound, Restore defaults for all.
-- The commands' keys follow the bindings (keys.key), so a rebind works at
-- once, menu bar and all, with no reload.

local ds = require("@ds")
local Shortcut = require("@ds/shortcut_recorder")
local search = require("@app/screens/settings/search")

local M = {}

M.ID = "shortcuts"
M.TITLE = "Shortcuts"

export type Key = { id: string, title: string, key: string?, group: string?, menu: string? }

-- The shell's table, or none while it isn't there yet.
local function keys(): { Key }
	local ok, shell_keys = pcall(require, "@app/screens/shell/keys")
	local list = if ok and type(shell_keys) == "table" then (shell_keys :: any).LIST else nil
	return if type(list) == "table" then list :: { Key } else {}
end

-- The shell's names for a shortcut and a group in the locale shown
-- (keys.title, keys.group_title: read where they're drawn), else as given.
local function shell_keys(): any
	local ok, k = pcall(require, "@app/screens/shell/keys")
	return if ok and type(k) == "table" then k else nil
end
local function title_of(k: Key): string
	local s = shell_keys()
	return if s and s.title then s.title(k) else k.title
end
local function group_of(group: string): string
	local s = shell_keys()
	return if s and s.group_title then s.group_title(group) else group
end
-- The same as messages, for the search (read in the locale it's run in): a
-- shortcut's, where the catalog has one, else its English.
local function title_message(k: Key): string | Message
	local n = string.match(k.id, "^shell%.session_(%d+)$")
	if n then
		return ui.t("shell.keys.title.session_n", { i = n })
	end
	local key = `shell.keys.title.{(string.gsub(k.id, "%.", "_"))}`
	-- (Asked once, to know it's there: not drawn.)
	return if ui.t(key):get() ~= key then ui.t(key) else k.title
end
local GROUP_KEYS: { [string]: string } = {
	Sessions = "shell.keys.group.sessions",
	Panels = "shell.keys.group.panels",
	Go = "shell.keys.group.go",
	App = "shell.keys.group.app",
}
local function group_message(group: string?): (string | Message)?
	local key = if group then GROUP_KEYS[group] else nil
	return if key then ui.t(key) else group
end

-- The groups, in the shipping app's order; any other after them.
local ORDER = { "Sessions", "Panels", "Go", "App" }

local function grouped(): { { name: string, keys: { Key } } }
	local by: { [string]: { Key } } = {}
	local names: { string } = {}
	for _, k in keys() do
		if k.key then
			local g = k.group or "App"
			if not by[g] then
				by[g] = {}
				table.insert(names, g)
			end
			table.insert(by[g], k)
		end
	end
	local out = {}
	local seen: { [string]: boolean } = {}
	for _, g in ORDER do
		if by[g] then
			table.insert(out, { name = g, keys = by[g] })
			seen[g] = true
		end
	end
	for _, g in names do
		if not seen[g] then
			table.insert(out, { name = g, keys = by[g] })
		end
	end
	return out
end

-- Each shortcut, searchable by its name and its group.
do
	local rows = {}
	for _, k in keys() do
		if k.key then
			table.insert(rows, { key = `key_{k.id}`, title = title_message(k), section = group_message(k.group), terms = ui.t("settings.shortcuts.row_terms") })
		end
	end
	table.insert(rows, {
		key = "key_appshot",
		title = ui.t("settings.shortcuts.appshot.label"),
		section = ui.t("settings.shortcuts.appshot.section"),
		terms = ui.t("settings.shortcuts.appshot.terms"),
	})
	search.index(M.ID, ui.t("settings.shortcuts.title"), ui.t("settings.shortcuts.terms"), rows)
end

-- The Appshot hotkey (screens/chat/appshots): the host's, working while
-- another app is in front, not a menu command, so not in the shell's table.
-- Shown while Appshots are on, as it stands; changed in Settings → Appshots.
local function appshot(): Node?
	local ok, appshots = pcall(require, "@app/screens/chat/appshots")
	if not ok or type(appshots) ~= "table" or not (appshots :: any).shortcut then
		return nil
	end
	local a = appshots :: any
	return ui.show(a.enabled, ds.Settings.Section {
		title = ui.t("settings.shortcuts.appshot.section") :: any,
		ds.Settings.Group {
			search.anchor("key_appshot", ds.Settings.Row {
				label = ui.t("settings.shortcuts.appshot.label") :: any,
				description = ui.t("settings.shortcuts.appshot.description") :: any,
				control = ui.row {
					gap = "$space.2",
					items = "center",
					-- (The keys as set: a new one is a new Kbd.)
					ui.presence(function(): string
						return a.shortcut:get()
					end, function(keys: string): Node
						return ds.Kbd { keys = keys }
					end),
					ds.Button {
						ui.t("settings.shortcuts.appshot.change"),
						tone = "ghost",
						size = "sm",
						on_click = function()
							require("@app/screens/shell/route").settings("appshots")
						end,
					},
				},
			}),
		},
	})
end

-- The shortcut being recorded (its id), and what the last refused one said.
local recording = ui.state("settings_shortcuts_recording", nil :: string?)
local notice = ui.state("settings_shortcuts_notice", nil :: string?)

-- Who holds a combination, by name: a shortcut of the shell's in the
-- locale shown, else the command's id (a plugin's).
local function owner_name(owner: string): string
	local s = shell_keys()
	local k = if s then s.get(owner) else nil
	return if k then title_of(k) else owner
end

-- `id`'s keys: Reset while rebound, then its recorder.
local function binding(k: Key): Node
	local s = shell_keys()
	if not (s and s.combo) then
		return ds.Kbd { keys = k.key :: string }
	end
	local id = k.id
	return ui.row {
		gap = "$space.2",
		items = "center",
		ui.show(function(): boolean
			return s.customized(id)
		end, ds.Button {
			ui.t("settings.shortcuts.reset"),
			tone = "ghost",
			size = "sm",
			label = function(): string
				return ui.t("settings.shortcuts.reset_label", { name = title_of(k), keys = Shortcut.shown(k.key :: string) }):get()
			end,
			on_click = function()
				notice:set(nil)
				s.reset(id)
			end,
		}),
		-- (While it records, every key comes to it, a command's ⌘B included:
		-- ds/shortcut_recorder's `capture_keys`.)
		Shortcut.Recorder {
			value = function(): string
				return s.combo(id)
			end,
			label = function(): string
				return title_of(k)
			end :: any,
			on_listening = function(on: boolean)
				if on then
					notice:set(nil)
					recording:set(id)
				elseif recording:peek() == id then
					recording:set(nil)
				end
			end,
			on_change = function(combo: string)
				local owner = s.owner(combo, id)
				if owner then
					notice:set(ui.t("settings.shortcuts.taken", { keys = Shortcut.shown(combo), name = owner_name(owner) }):get())
					return
				end
				notice:set(nil)
				s.bind(id, combo)
			end,
		},
	}
end

-- Restore defaults: every shortcut back as Zeron has it, shown while any
-- is rebound.
local function restore(): Node?
	local s = shell_keys()
	if not (s and s.reset_all) then
		return nil
	end
	return ui.show(function(): boolean
		return next(s.bound:get()) ~= nil
	end, ds.Button {
		tone = "ghost",
		size = "sm",
		ds.Icon { name = "restart", size = 14 },
		ui.t("settings.shortcuts.restore"),
		on_click = function()
			notice:set(nil)
			s.reset_all()
		end,
	})
end

function M.view(): Node
	local sections: { Node } = {}
	for _, g in grouped() do
		local rows: { Node } = {}
		for _, k in g.keys do
			table.insert(rows, search.anchor(`key_{k.id}`, ds.Settings.Row {
				label = function(): string
					return title_of(k)
				end,
				control = binding(k),
			}))
		end
		local name = g.name
		table.insert(sections, ds.Settings.Section {
			title = ui.computed(function(): string
				return group_of(name)
			end) :: any,
			ds.Settings.Group(rows :: any),
		})
	end
	local appshot_section = appshot()
	if appshot_section then
		table.insert(sections, appshot_section)
	end
	if #sections == 0 then
		table.insert(sections, ds.EmptyState {
			icon = "keyboard",
			title = ui.t("settings.shortcuts.empty_title") :: any,
			body = ui.t("settings.shortcuts.empty_body") :: any,
			size = "sm",
		})
	end
	return ds.Settings.Page {
		title = ui.t("settings.shortcuts.title") :: any,
		-- (While one records, how to stop; a refusal, whose it is.)
		description = function(): string
			if recording:get() then
				return ui.t("settings.shortcuts.recording"):get()
			end
			return notice:get() or ui.t("settings.shortcuts.description"):get()
		end :: any,
		trail = restore(),
		table.unpack(sections),
	}
end

return M

--!strict
-- Your theme library, in Settings → Appearance: VS Code colour themes you
-- import (a theme's `.json`, comments allowed, or an extension's
-- `package.json`), compiled by the shipping app's theme crate (src/themes.rs)
-- into palettes the window wears (ds/palettes). They're listed in the Light
-- and Dark theme choices under "Imported", after Zeron's "Built in", and
-- each can be renamed or removed here. Removing the one in use puts Zeron's
-- own back.
--
--   library.options(kind)   the Light or Dark theme choices, grouped (a function)
--   library.import(path)    import a file (what Import theme… does once picked)
--   library.Section()       the import button, an import's error, your themes

local ds = require("@ds")
local Menu = require("@ds/menu")
local palettes = require("@ds/palettes")
local locale = require("@ds/locale")

local M = {}

-- An import's error, in the shipping app's words, until the next import.
M.error = ui.state("themes_import_error", nil :: string?)
M.busy = ui.state("themes_importing", false)

local renaming = ui.state("themes_renaming", nil :: string?)
local renaming_open = ui.state("themes_renaming_open", false)
local new_name = ui.state("themes_new_name", "")
local removing = ui.state("themes_removing", nil :: string?)
local removing_open = ui.state("themes_removing_open", false)

-- What the host does (src/themes.rs), where a test may stand in for it:
-- import and rename are promises (`:after(ok, err)`), remove isn't.
M.host = {
	import = function(path: string): any
		return ui.call("themes.import", path)
	end,
	remove = function(id: string)
		ui.invoke("themes.remove", id)
	end,
	rename = function(id: string, name: string): any
		return ui.call("themes.rename", { id = id, name = name })
	end,
}

-- Zeron's own for each scheme: what a removed theme gives way to.
local OWN = { light = "zeron-light", dark = "zeron-dark" }

-- The Light or Dark theme choices: Zeron's built-in palettes, then yours.
function M.options(kind: string): () -> { any }
	return function(): { any }
		local out = {}
		for _, p in palettes.LIST do
			if p.kind == kind then
				table.insert(out, { value = p.id, label = p.name, group = ui.t("settings.theme_library.built_in"):get() })
			end
		end
		for _, p in palettes.imported() do
			if p.kind == kind then
				table.insert(out, { value = p.id, label = p.name, group = ui.t("settings.theme_library.imported"):get() })
			end
		end
		return out
	end
end

local function said(err: any): string
	if type(err) == "table" and err.message then
		return tostring(err.message)
	end
	return tostring(err)
end

-- `path` into the library. Its error stays on the page until the next try.
function M.import(path: string)
	M.error:set(nil)
	M.busy:set(true)
	M.host.import(path):after(function(done: any)
		M.busy:set(false)
		local count = if done and done.variants then #done.variants else 0
		ds.toast.show {
			title = ui.t("settings.theme_library.imported_toast", { count = count }):get(),
			body = ui.t("settings.theme_library.imported_body"):get(),
			tone = "success",
		}
		if done and done.skipped and #done.skipped > 0 then
			M.error:set(ui.t("settings.theme_library.skipped", { names = table.concat(done.skipped, "; ") }):get())
		end
	end, function(err: any)
		M.busy:set(false)
		M.error:set(said(err))
	end)
end

-- Pick a file, then import it.
local function pick()
	ui.task(function()
		local files = ui.open_files({
			title = ui.t("settings.theme_library.choose_source"):get(),
			filters = { { ui.t("settings.theme_library.vscode_theme"):get(), { "json", "jsonc" } } },
		}):await()
		local file = files and files[1]
		if file then
			M.import(file.path)
		end
	end)
end

-- Entry `id` out of the library: any of its palettes in use gives way to
-- Zeron's own first.
function M.remove(id: string)
	for _, entry in palettes.library():peek().entries or {} do
		if entry.id == id then
			for _, variant in entry.variants or {} do
				for kind, chosen in ds.look.palette do
					if chosen:peek() == variant then
						chosen:set(OWN[kind])
					end
				end
			end
		end
	end
	M.host.remove(id)
end

function M.rename(id: string, name: string): any
	return M.host.rename(id, name)
end

-- An imported theme's swatch: its background with its accent and text,
-- following `palette()` in place.
local function swatch(palette: () -> any): Node
	local function color(field: string): () -> string
		return function(): string
			local p = palette()
			return ui.raw(if p then p[field] else "#00000000", "the theme's own colour")
		end
	end
	return ui.row {
		w = 32,
		h = 32,
		shrink = 0,
		rounded = "$radius.md",
		border = 1,
		border_color = "$border",
		overflow = "hidden",
		items = "center",
		justify = "center",
		gap = "$zeron.space.hair",
		bg = color("background"),
		ui.div { w = 6, h = 14, rounded = "$radius.full", bg = color("accent") },
		ui.div { w = 6, h = 10, rounded = "$radius.full", bg = color("text") },
	}
end

local function entry_row(_e: any, _i: number, now: State<any>): Node
	local id = now:peek().id
	local menu_open = ui.use_state(false)
	local more = ds.IconButton {
		icon = "more-horizontal",
		label = function(): string
			return ui.t("settings.theme_library.more_for", { name = now:get().name }):get()
		end,
		size = "sm",
		tone = "quiet",
		on_click = function()
			menu_open:set(not menu_open:peek())
		end,
	}
	local function first(): any
		local want = (now:get().variants or {})[1]
		return if want then palettes.find(want) else nil
	end
	return ds.Settings.Row {
		label = function(): string
			return now:get().name
		end,
		description = function(): string
			local e = now:get()
			if e.status and e.status.ok == false then
				return e.status.message or ui.t("settings.theme_library.needs_look"):get()
			end
			local kinds = {}
			for _, v in e.variants or {} do
				local p = palettes.find(v)
				if p then
					table.insert(kinds, if p.kind == "dark" then ui.t("settings.theme_library.dark"):get() else ui.t("settings.theme_library.light"):get())
				end
			end
			return table.concat(kinds, " · ")
		end,
		lead = swatch(first),
		truncate = true,
		control = ui.menu(Menu.panel {
			open = menu_open,
			anchor = more,
			placement = { side = "bottom", align = "end", offset = 4 },
			w = 200,
			Menu.Group {
				Menu.Row {
					icon = "pen",
					-- (A string, read as the menu is made: Menu.Row names
					-- itself only by one.)
					label = locale.s("settings.theme_library.rename"),
					role = "menuitem",
					on_click = function()
						menu_open:set(false)
						renaming:set(id)
						new_name:set(now:peek().name)
						renaming_open:set(true)
					end,
				},
			},
			Menu.Rule {},
			Menu.Group {
				Menu.Row {
					icon = "trash-bin-minimalistic",
					label = locale.s("settings.theme_library.remove"),
					role = "menuitem",
					danger = true,
					on_click = function()
						menu_open:set(false)
						removing:set(id)
						removing_open:set(true)
					end,
				},
			},
		}),
	}
end

-- The import, its error, and your themes: a section of Appearance.
function M.Section(): Node
	local entries = ui.computed(function(): { any }
		return palettes.library():get().entries or {}
	end)
	return ds.Settings.Section {
		title = ui.t("settings.theme_library.section") :: any,
		description = ui.t("settings.theme_library.section_description") :: any,
		trail = ds.Button {
			tone = "secondary",
			size = "sm",
			disabled = function(): boolean
				return M.busy:get()
			end,
			on_click = pick,
			ui.t("settings.theme_library.import"),
		},
		ui.show(function(): boolean
			return M.error:get() ~= nil
		end, ds.Callout {
			tone = "danger",
			title = ui.t("settings.theme_library.import_failed") :: any,
			body = function(): string
				return M.error:get() or ""
			end,
		}),
		ds.Settings.Each(entries, entry_row, {
			key = function(e: any): string
				return e.id
			end,
		}),
		ds.Dialog {
			open = renaming_open,
			title = ui.t("settings.theme_library.rename_title"),
			size = "sm",
			confirm = {
				label = ui.t("settings.theme_library.rename") :: any,
				on_click = function()
					local id, name = renaming:peek(), string.match(new_name:peek(), "^%s*(.-)%s*$") or ""
					renaming:set(nil)
					if id and name ~= "" then
						M.rename(id, name):after(function() end, function(err: any)
							ds.toast.show { title = ui.t("settings.theme_library.couldnt_rename"):get(), body = said(err), tone = "warning" }
						end)
					end
				end,
			},
			ds.TextField { value = new_name, label = ui.t("settings.theme_library.name"), w_full = true, select_on_focus = true },
		},
		ds.Dialog {
			open = removing_open,
			title = ui.t("settings.theme_library.remove_title"),
			body = ui.t("settings.theme_library.remove_body"),
			tone = "danger",
			confirm = {
				label = ui.t("settings.theme_library.remove") :: any,
				on_click = function()
					local id = removing:peek()
					removing:set(nil)
					if id then
						M.remove(id)
					end
				end,
			},
		},
	}
end

return M

--!strict
-- Settings → Themes: every theme Zeron can wear, laid out as Plugins and
-- Templates are (ds.Settings.Catalog): the same search, tabs, card of rows
-- and empty states. A theme is only colours, so it's worn with whichever
-- template runs, and it needs no allowing: it's data (src/themes.rs).
--
-- Its tabs: all of them; Zeron's built in; those templates bring (a
-- template's own colours, worn where you haven't chosen a theme); and
-- yours, imported from VS Code (a theme's .json, or an extension's folder,
-- by Import theme or dropped on the page).
--
-- Each row is its theme's as the list changes: its swatch (ds/theme_swatch,
-- a half for each scheme it has), its name, which schemes it has, where it
-- came from, and what it's worn for now ("In use", or "Light" or "Dark"
-- while following the system with another theme for the other); Use (worn
-- for each scheme it has) or nothing once it's worn for all it can be; and a
-- ⋯ menu: Use only for light or for dark, for one with both; a template's,
-- Show in Templates; yours, Rename and Remove. What's at a row's end swaps
-- only when its use does.

local ds = require("@ds")
local Menu = require("@ds/menu")
local control = require("@ds/control")
local DropOverlay = require("@ds/drop_overlay")
local ThemeSwatch = require("@ds/theme_swatch")
local state = require("@app/screens/themes/state")
local templates = require("@app/screens/templates/state")
local route = require("@app/screens/shell/route")

local M = {}

local tab = ui.state("themes_tab", "all")
local query = ui.state("themes_query", "")
local renaming = ui.state("themes_renaming", nil :: string?)
local renaming_open = ui.state("themes_renaming_open", false)
local new_name = ui.state("themes_new_name", "")
local removing = ui.state("themes_removing", nil :: string?)
local removing_open = ui.state("themes_removing_open", false)
-- Files carried over the page from outside.
local carrying = ui.state("themes_carrying", false)

local TABS = { builtin = "builtin", templates = "template", yours = "imported" }

local function template_name(id: string?): string
	for _, t in templates.list:get() do
		if t.id == id then
			return t.name
		end
	end
	return id or ""
end

-- Which schemes it has, or what's wrong with an imported one.
local function about(e: state.Entry): string
	if e.status and e.status.ok == false then
		return e.status.message or ui.t("themes.page.needs_fixing"):get()
	end
	if e.id == "zeron" then
		return ui.t("themes.page.about.default"):get()
	end
	local light, dark = state.has(e, "light") ~= nil, state.has(e, "dark") ~= nil
	local key = if light and dark then "both" elseif light then "light" else "dark"
	return ui.t(`themes.page.about.{key}`):get()
end

-- Where it came from, on its own line.
local function byline(e: state.Entry): string
	if e.source == "template" then
		return ui.t("themes.page.byline.template", { name = template_name(e.template) }):get()
	elseif e.source == "imported" then
		return ui.t("themes.page.byline.imported"):get()
	end
	return ui.t("themes.page.byline.builtin"):get()
end

local function matches(e: state.Entry, q: string): boolean
	if q == "" then
		return true
	end
	return string.find(string.lower(`{e.name} {byline(e)}`), q, 1, true) ~= nil
end

-- The themes the tab holds that the search finds.
local found = ui.computed(function(): { state.Entry }
	local q = string.lower(query:get())
	local source = TABS[tab:get()]
	local out: { state.Entry } = {}
	for _, e in state.entries:get() do
		if (source == nil or e.source == source) and matches(e, q) then
			table.insert(out, e)
		end
	end
	return out
end)

local function count(which: string): () -> number
	return function(): number
		local source, n = TABS[which], 0
		for _, e in state.entries:get() do
			if source == nil or e.source == source then
				n += 1
			end
		end
		return n
	end
end

local function imported(done: any)
	local count_ = if done and done.variants then #done.variants else 0
	if count_ > 0 then
		tab:set("yours")
		ds.toast.show {
			title = ui.t("themes.page.imported_toast", { count = count_ }):get(),
			body = ui.t("themes.page.imported_body"):get(),
			tone = "success",
		}
	end
end

-- `path` brought in, said once it's in (or its error over the card).
local function import(path: string)
	local p: any = state.import(path)
	if p ~= nil then
		p:after(imported, function(_: any) end)
	end
end

-- Pick a file, then import it.
local function pick()
	ui.task(function()
		local files = ui.open_files({
			title = ui.t("themes.page.choose_source"):get(),
			filters = { { ui.t("themes.page.vscode_theme"):get(), { "json", "jsonc" } } },
		}):await()
		local file = files and files[1]
		if file then
			import(file.path)
		end
	end)
end

-- Files dropped on the page: each brought in (a folder as an extension).
function M.dropped(files: { any })
	carrying:set(false)
	for _, f in files do
		if type(f) == "table" and type(f.path) == "string" then
			import(f.path)
		end
	end
end

-- What it's worn for now, after its name.
local function badge(e: state.Entry): string?
	local fills = state.fills(e)
	local showing = state.showing()
	if #showing == 1 then
		return if fills[showing[1]] then ui.t("themes.page.in_use"):get() else nil
	elseif fills.light and fills.dark then
		return ui.t("themes.page.in_use"):get()
	elseif fills.light then
		return ui.t("themes.page.for_light"):get()
	elseif fills.dark then
		return ui.t("themes.page.for_dark"):get()
	end
	return nil
end

-- Use and its empty slot take the same width, the wider as laid out, so a
-- row's end never shifts as its use changes (as Templates').
local slot = ui.state("themes_use_w", 0)
local function slot_w(): number
	return slot:get()
end
local function grow(size: any)
	if type(size) == "table" and type(size.w) == "number" and size.w > slot:peek() then
		slot:set(size.w)
	end
end

-- Use, or its slot kept empty; then its ⋯ menu (or the menu's room, for a
-- theme with nothing more to do, so the column stays one down the list).
local function controls(e: state.Entry, now: State<any>): Node
	local id = e.id
	local menu_open = ui.use_state(false)
	local function pick_(run: () -> ()): () -> ()
		return function()
			menu_open:set(false)
			run()
		end
	end
	local groups: { Node } = {}
	local light, dark = state.has(e, "light"), state.has(e, "dark")
	if light and dark then
		table.insert(groups, Menu.Group {
			Menu.Row {
				icon = "sun",
				label = ui.t("themes.page.use_light"),
				role = "menuitem",
				on_click = pick_(function()
					state.use_for("light", light)
				end),
			},
			Menu.Row {
				icon = "moon",
				label = ui.t("themes.page.use_dark"),
				role = "menuitem",
				on_click = pick_(function()
					state.use_for("dark", dark)
				end),
			},
		})
	end
	if e.source == "template" then
		table.insert(groups, Menu.Group {
			Menu.Row {
				icon = "gallery",
				label = ui.t("themes.page.show_template"),
				role = "menuitem",
				on_click = pick_(function()
					route.settings("templates")
				end),
			},
		})
	elseif e.source == "imported" then
		table.insert(groups, Menu.Group {
			Menu.Row {
				icon = "pen",
				label = ui.t("themes.page.rename"),
				role = "menuitem",
				on_click = pick_(function()
					renaming:set(id)
					new_name:set(now:peek().name)
					renaming_open:set(true)
				end),
			},
		})
		table.insert(groups, Menu.Group {
			Menu.Row {
				icon = "trash-bin-minimalistic",
				label = ui.t("themes.page.remove"),
				role = "menuitem",
				danger = true,
				on_click = pick_(function()
					removing:set(id)
					removing_open:set(true)
				end),
			},
		})
	end
	local more: Node
	if #groups == 0 then
		more = ui.div { w = "$zeron.icon_button.sm", h = "$zeron.icon_button.sm", shrink = 0 }
	else
		local anchor = ds.IconButton {
			icon = "more-horizontal",
			label = function(): string
				return ui.t("themes.page.more_for", { name = now:get().name }):get()
			end,
			size = "sm",
			tone = "quiet",
			on_click = function()
				menu_open:set(not menu_open:peek())
			end,
		}
		local panel: { [any]: any } = { open = menu_open, anchor = anchor, placement = { side = "bottom", align = "end", offset = 4 }, w = 220 }
		for i, g in groups do
			if i > 1 then
				table.insert(panel, Menu.Rule {})
			end
			table.insert(panel, g)
		end
		-- (The menu draws its anchor, the ⋯, in place.)
		more = ui.menu(Menu.panel(panel))
	end
	return ui.row {
		gap = "$space.2",
		items = "center",
		ui.switch(function(): string
			return if state.full(now:get()) then "using" else "use"
		end, {
			using = ui.div { min_w = slot_w, h = control.H.sm },
			use = ds.Button {
				tone = "secondary",
				size = "sm",
				min_w = slot_w,
				on_resize = grow,
				on_click = function()
					state.use(now:peek())
				end,
				ui.t("themes.page.use"),
			},
		}),
		more,
	}
end

local function row(e: state.Entry, _i: number, now: State<any>): Node
	return ui.column {
		id = `theme-{e.id}`,
		class = "theme-row",
		w_full = true,
		ds.Settings.Row {
			label = function(): string
				return now:get().name
			end,
			description = function(): string
				return about(now:get())
			end,
			byline = function(): string
				return byline(now:get())
			end,
			lead = ThemeSwatch.Swatch {
				themes = function(): { string }
					return state.previews:get()[e.id] or {}
				end,
				class = "theme-swatch",
			},
			badge = ui.show(function(): boolean
				return badge(now:get()) ~= nil
			end, ds.Badge {
				function(): string
					return badge(now:get()) or ""
				end,
				tone = "accent",
				class = "theme-in-use",
			}),
			truncate = true,
			control = controls(e, now),
		},
	}
end

-- What a tab with nothing in it says (All and Built in always hold Zeron's).
local function none(which: string): Node
	if which == "templates" then
		return ds.EmptyState {
			icon = "gallery",
			title = ui.t("themes.page.none_templates"),
			body = ui.t("themes.page.none_templates_body"),
			size = "sm",
		}
	end
	return ds.EmptyState {
		icon = "palette",
		title = ui.t("themes.page.none_yours"),
		body = ui.t("themes.page.none_yours_body"),
		size = "sm",
		action = { label = ui.t("themes.page.import"), on_click = pick },
	}
end

-- An import's error, over the card until the next one.
local function notice(): Node
	return ds.Callout {
		tone = "danger",
		title = ui.t("themes.page.import_failed") :: any,
		body = function(): string
			return state.error:get() or ""
		end,
	}
end

local function dialogs(): { Node }
	return {
		ds.Dialog {
			open = renaming_open,
			title = ui.t("themes.page.rename_title"),
			size = "sm",
			confirm = {
				label = ui.t("themes.page.rename") :: any,
				on_click = function()
					local id, name = renaming:peek(), string.match(new_name:peek(), "^%s*(.-)%s*$") or ""
					renaming:set(nil)
					if id and name ~= "" then
						local p: any = state.rename(id, name)
						if p ~= nil then
							p:after(function() end, function(err: any)
								ds.toast.show { title = ui.t("themes.page.couldnt_rename"):get(), body = state.said(err), tone = "warning" }
							end)
						end
					end
				end,
			},
			ds.TextField { value = new_name, label = ui.t("themes.page.name"), w_full = true, select_on_focus = true },
		},
		ds.Dialog {
			open = removing_open,
			title = ui.t("themes.page.remove_title"),
			body = ui.t("themes.page.remove_body"),
			tone = "danger",
			confirm = {
				label = ui.t("themes.page.remove") :: any,
				on_click = function()
					local id = removing:peek()
					removing:set(nil)
					if id then
						state.remove(id)
					end
				end,
			},
		},
	}
end

function M.View(): Node
	-- The swatches' themes, kept while the page is open.
	state.swatches()
	return ui.column {
		id = "themes-page",
		w_full = true,
		items = "center",
		relative = true,
		-- A theme file or an extension's folder dropped anywhere on it.
		on_file_hover = function(inside: boolean)
			carrying:set(inside)
		end,
		on_file_drop = M.dropped,
		ds.Settings.Catalog {
			title = ui.t("themes.page.title"),
			description = ui.t("themes.page.description"),
			search = { value = query, placeholder = ui.t("themes.page.search") },
			tabs = {
				value = tab,
				label = ui.t("themes.page.tabs"),
				options = {
					{ value = "all", label = ui.t("themes.page.all"), count = count("all") },
					{ value = "builtin", label = ui.t("themes.page.builtin"), count = count("builtin") },
					{ value = "templates", label = ui.t("themes.page.from_templates"), count = count("templates") },
					{ value = "yours", label = ui.t("themes.page.yours"), count = count("yours") },
				},
			},
			import = {
				label = ui.t("themes.page.import"),
				on_click = pick,
			},
			notice = notice,
			notice_when = function(): boolean
				return state.error:get() ~= nil
			end,
			status = function(): string
				return if state.ready() then "ready" else "loading"
			end,
			found = found,
			row = row,
			key = function(e: state.Entry): string
				return e.id
			end,
			none = none,
			hint = { id = "themes-hint", text = ui.t("themes.page.hint") },
			overlays = dialogs(),
		},
		DropOverlay { shown = function(): boolean
			return carrying:get()
		end, id = "themes-drop", label = ui.t("themes.page.drop") :: any },
	}
end

return M

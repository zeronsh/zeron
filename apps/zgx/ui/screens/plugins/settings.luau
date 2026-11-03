--!strict
-- Plugins' settings: a page each in Settings, under "Plugins" after
-- Zeron's own pages, for every plugin that's on and declares settings in
-- its manifest (`settings`, src/plugins/settings.rs). Zeron draws each
-- page with the same Page, Section, Group and Row every built-in page
-- uses: a plugin declares, it never draws here, and no plugin reaches
-- these pages (`private`). The host publishes the list (`plugins.settings`,
-- the app's alone), by name:
--
--   { { plugin, name, settings = { Setting }, values = { [id]: value } } }
--
-- A setting is `{ id, kind, label, description?, section?, default }` and,
-- by kind: a choice's `options` ({ value, label }) and `style` ("select" or
-- "segmented"); text's `placeholder`; a number's `min`, `max`, `step`,
-- `unit`. Kinds: switch, choice, text, number, shortcut, folder, and
-- `permission` (Zeron's own plugins alone, `{ id, kind, permission }`: a
-- macOS permission's row, every word and button Zeron's, screens/plugins/
-- permissions). An option may carry a `description`, said in the
-- setting's place while it's chosen.
--
-- `status` holds what the plugin reports of its settings, by id (`{ badge?,
-- tone, text? }`, plain text in a fixed tone): Zeron draws the badge beside
-- the control and the text in the description's place.
--
-- What's set goes to the host (`M.set`, `M.choose_folder`: a fixture
-- answers them itself), which keeps it for that plugin alone and tells
-- the plugin (`plugin.<id>.settings`). Each page is in Settings search.

local ds = require("@ds")
local shell = require("@app/screens/shell/settings")
local search = require("@app/screens/settings/search")
local route = require("@app/screens/shell/route")
local words = require("@app/screens/plugins/words")
local Shortcut = require("@ds/shortcut_recorder")
local permissions = require("@app/screens/plugins/permissions")
local appearance = require("@app/screens/shell/appearance")
local wallpaper = require("@app/screens/settings/wallpaper")

local M = {}

export type Option = { value: string, label: string, description: string? }
export type Status = { badge: string?, tone: string?, text: string? }
export type Setting = {
	id: string,
	kind: string,
	-- (None on a permission row: Zeron's words.)
	label: string?,
	permission: string?,
	description: string?,
	section: string?,
	default: any,
	options: { Option }?,
	style: string?,
	placeholder: string?,
	min: number?,
	max: number?,
	step: number?,
	unit: string?,
}
export type Entry = {
	plugin: string,
	name: string,
	settings: { Setting },
	values: { [string]: any },
	status: { [string]: Status }?,
	-- What its page says under its title (the manifest's
	-- `settings_description`, plain); none, Zeron's own line.
	description: string?,
}

M.list = ui.shared("plugins.settings", {} :: { Entry })

-- A plugin's page, by its id.
function M.page_id(plugin: string): string
	return `plugin-{plugin}`
end

-- A plugin's entry now (followed where it's read), or nil: it's off, or
-- has no settings.
function M.of(plugin: string): Entry?
	for _, e in M.list:get() do
		if e.plugin == plugin then
			return e
		end
	end
	return nil
end

function M.has(plugin: string): boolean
	return M.of(plugin) ~= nil
end

-- Its page, opened.
function M.open(plugin: string)
	route.settings(M.page_id(plugin))
end

-- What a page asks of the host.
function M.set(plugin: string, id: string, value: any)
	ui.invoke("plugins.setting_set", { plugin = plugin, id = id, value = if value == nil then ui.null else value })
end

function M.choose_folder(plugin: string, id: string, title: string): any
	return ui.call("plugins.setting_folder", { plugin = plugin, id = id, title = title })
end

-- A setting's value now, followed.
local function value_of(plugin: string, id: string): any
	local e = M.of(plugin)
	return if e then e.values[id] else nil
end

-- What the plugin reports of a setting now (followed), or nil.
local function status_of(plugin: string, id: string): Status?
	local e = M.of(plugin)
	local st = if e and e.status then e.status[id] else nil
	return if type(st) == "table" then st else nil
end

-- A setting's label: its own, or Zeron's for a permission or the wallpaper.
local function label_of(s: Setting): string
	if s.kind == "wallpaper" then
		return ui.t("plugins.settings.wallpaper.label"):get()
	end
	if s.kind == "permission" then
		local p = permissions.KNOWN[s.permission or ""]
		return if p then p.title else s.id
	end
	return s.label or s.id
end

-- What a row says under its label, as it changes: what the plugin reports,
-- else the chosen option's own words, else the setting's.
local function description_of(plugin: string, s: Setting): () -> string
	return function(): string
		local st = status_of(plugin, s.id)
		if st and st.text and st.text ~= "" then
			return st.text
		end
		if s.kind == "choice" then
			local chosen = value_of(plugin, s.id)
			for _, o in s.options or {} do
				if o.value == chosen and o.description then
					return o.description
				end
			end
		end
		return s.description or ""
	end
end

-- The badge a plugin reports beside a control, while it reports one.
local TONES = { neutral = true, success = true, warning = true, danger = true }
local function status_badge(plugin: string, id: string): Node
	local function badge(): string
		local st = status_of(plugin, id)
		return if st and st.badge then st.badge else ""
	end
	return ui.show(function(): boolean
		return badge() ~= ""
	end, ds.Badge {
		badge,
		tone = function(): any
			local st = status_of(plugin, id)
			local tone = if st then st.tone else nil
			return if tone and TONES[tone] then tone else "neutral"
		end,
	} :: any)
end

-- A permission's row control, every part Zeron's: granted or not, the way
-- to grant it (macOS's prompt, then System Settings), and Check again.
local PermissionControl = ui.component(function(props: { permission: string }): Node
	local p = permissions.KNOWN[props.permission]
	local asked = ui.use_state(false)
	local function granted(): boolean
		return permissions.granted(props.permission)
	end
	return ui.row {
		gap = "$space.2",
		items = "center",
		ui.show(granted, ds.Badge { ui.t("plugins.settings.granted"), tone = "success" }, ds.Badge { ui.t("plugins.settings.required"), tone = "warning" }),
		ui.show(function(): boolean
			return not granted()
		end, ui.row {
			gap = "$space.2",
			items = "center",
			ds.Button {
				tone = "secondary",
				size = "sm",
				on_click = function()
					permissions.request(props.permission)
					asked:set(true)
				end,
				ui.text(function(): string
					return if asked:get() then ui.t("plugins.settings.open_system_settings"):get() else p.grant
				end),
			} :: any,
			ds.Button {
				tone = "ghost",
				size = "sm",
				label = function(): string
					return ui.t("plugins.settings.check_named", { name = p.title }):get()
				end,
				on_click = permissions.refresh,
				ui.t("plugins.settings.check_again"),
			},
		}),
	}
end)

-- A state of a setting's value for a control: it follows the host's, and
-- what the person sets in it goes to the host.
local function bound(plugin: string, s: Setting): State<any>
	local now = ui.use_state(value_of(plugin, s.id))
	ui.effect(function()
		local held = value_of(plugin, s.id)
		if held ~= nil and now:peek() ~= held then
			now:set(held)
		end
	end)
	ui.effect(function()
		local v = now:get()
		if v ~= nil and v ~= value_of(plugin, s.id) then
			M.set(plugin, s.id, v)
		end
	end)
	return now
end

-- A number as a field shows it: whole ones without a point.
local function number_text(n: any): string
	if type(n) ~= "number" then
		return ""
	end
	return if n == math.floor(n) then string.format("%d", n) else tostring(n)
end

-- Why a typed number won't do, or nil.
local function number_problem(s: Setting, text: string): string?
	local trimmed = string.match(text, "^%s*(.-)%s*$") or ""
	local n = tonumber(trimmed)
	if n == nil then
		return ui.t("plugins.settings.a_number"):get()
	elseif s.min and n < s.min then
		return ui.t("plugins.settings.at_least", { n = number_text(s.min) }):get()
	elseif s.max and n > s.max then
		return ui.t("plugins.settings.at_most", { n = number_text(s.max) }):get()
	end
	return nil
end

-- The control at a row's end, by its setting's kind.
local Control = ui.component(function(props: { plugin: string, setting: Setting, name: string }): Node
	local plugin, s = props.plugin, props.setting
	if s.kind == "permission" then
		if not permissions.KNOWN[s.permission or ""] then
			return ui.div {}
		end
		-- (Read afresh as the page opens.)
		ui.on_mount(permissions.refresh)
		return PermissionControl { permission = s.permission :: string }
	end
	local label = label_of(s)
	if s.kind == "switch" then
		return ds.Switch { value = bound(plugin, s), label = label }
	elseif s.kind == "choice" then
		local options = s.options or {}
		if s.style == "segmented" then
			return ds.Segmented { value = bound(plugin, s), options = options, label = label }
		end
		return ds.Select { value = bound(plugin, s), options = options, name = label, w = 168 }
	elseif s.kind == "text" then
		local text = ui.use_state(tostring(value_of(plugin, s.id) or ""))
		local pending: (() -> ())? = nil
		local function commit()
			if pending then
				pending()
				pending = nil
			end
			if text:peek() ~= value_of(plugin, s.id) then
				M.set(plugin, s.id, text:peek())
			end
		end
		ui.effect(function()
			local held = value_of(plugin, s.id)
			if type(held) == "string" and pending == nil and text:peek() ~= held then
				text:set(held)
			end
		end)
		-- (Kept a moment after the typing stops, and at once on Enter or
		-- leaving the field.)
		ui.effect(function()
			local typed = text:get()
			if typed == value_of(plugin, s.id) then
				return
			end
			if pending then
				pending()
			end
			pending = ui.after(600, function()
				pending = nil
				commit()
			end)
		end)
		return ds.TextField {
			value = text,
			name = label,
			placeholder = s.placeholder,
			size = "sm",
			w = 220,
			on_commit = function()
				commit()
			end,
		}
	elseif s.kind == "number" then
		local text = ui.use_state(number_text(value_of(plugin, s.id)))
		local problem = ui.use_state(nil :: string?)
		ui.effect(function()
			local held = number_text(value_of(plugin, s.id))
			if text:peek() ~= held and problem:peek() == nil then
				text:set(held)
			end
		end)
		ui.effect(function()
			text:get()
			if problem:peek() ~= nil then
				problem:set(nil)
			end
		end)
		return ui.row {
			gap = "$space.2",
			items = "center",
			ds.TextField {
				value = text,
				name = label,
				size = "sm",
				w = 96,
				inputmode = "decimal",
				error = function(): string?
					return problem:get()
				end,
				on_commit = function(typed: string)
					local why = number_problem(s, typed)
					if why then
						problem:set(why)
						return
					end
					local n = tonumber((string.match(typed, "^%s*(.-)%s*$") or "")) :: number
					if n ~= value_of(plugin, s.id) then
						M.set(plugin, s.id, n)
					end
				end,
			},
			if s.unit then ui.text { s.unit, text_size = "$font.size.sm", color = "$text.secondary" } else nil,
		}
	elseif s.kind == "shortcut" then
		return Shortcut.Recorder {
			value = function(): string
				local v = value_of(plugin, s.id)
				return if type(v) == "string" then v else ""
			end,
			label = label,
			clearable = true,
			on_change = function(combo: string)
				M.set(plugin, s.id, combo)
			end,
		}
	elseif s.kind == "folder" then
		local function path(): string
			local v = value_of(plugin, s.id)
			return if type(v) == "string" then v else ""
		end
		return ui.row {
			gap = "$space.3",
			items = "center",
			min_w = 0,
			ui.text {
				function(): string
					local p = path()
					return if p == "" then ui.t("plugins.settings.none"):get() else (string.match(p, "([^/]+)/?$") or p)
				end,
				text_size = "$font.size.sm",
				color = "$text.secondary",
				truncate = true,
				max_w = 160,
			},
			ds.Button {
				tone = "secondary",
				size = "sm",
				label = ui.t("plugins.settings.choose_named", { name = label }),
				on_click = function()
					local p: any = M.choose_folder(plugin, s.id, ui.t("plugins.settings.folder_title", { plugin = props.name, setting = label }):get())
					if p ~= nil and p.after then
						p:after(function(_: any) end, function(err: any)
							ds.toast.show {
								title = ui.t("plugins.settings.couldnt_choose_folder"),
								body = if type(err) == "table" and err.message then tostring(err.message) else nil,
								tone = "warning",
							}
						end)
					end
				end,
				ui.t("plugins.settings.choose"),
			},
		}
	end
	return ui.div {}
end)

-- The settings in runs of their section (in the manifest's order).
local function sections(list: { Setting }): { { title: string?, settings: { Setting } } }
	local out: { { title: string?, settings: { Setting } } } = {}
	for _, s in list do
		local last = out[#out]
		if last and last.title == s.section then
			table.insert(last.settings, s)
		else
			table.insert(out, { title = s.section, settings = { s } })
		end
	end
	return out
end

-- What a page is made of, as a key: a plugin updated to declare other
-- settings is another page; values changing never are.
local function shape(e: Entry?): string
	if not e then
		return ""
	end
	local parts: { string } = {}
	for _, s in e.settings do
		local opts: { string } = {}
		for _, o in s.options or {} do
			table.insert(opts, `{o.value}={o.label}={o.description or ""}`)
		end
		table.insert(parts, table.concat({ s.id, s.kind, s.label or "", s.permission or "", s.description or "", s.section or "", s.style or "", s.unit or "", s.placeholder or "", table.concat(opts, ",") }, "\1"))
	end
	return table.concat(parts, "\0")
end

-- A plugin's wallpaper (a `wallpaper` row, Zeron's words): used as a new
-- chat's, in place of Appearance's (one plugin's at a time: turning it on
-- here takes over from another's), and Zeron's effect over it while it is.
local WallpaperRows = ui.component(function(props: { plugin: string, setting: Setting, name: string }): Node
	local plugin, s, name = props.plugin, props.setting, props.name
	local on = ui.use_state(appearance.plugin:peek() == plugin)
	ui.effect(function()
		local now = appearance.plugin:get() == plugin
		if on:peek() ~= now then
			on:set(now)
		end
	end)
	return ui.column {
		w_full = true,
		search.anchor(
			s.id,
			ds.Settings.Row {
				label = label_of(s),
				description = function(): string
					if appearance.plugin:get() == plugin then
						return ui.t("plugins.settings.wallpaper.on", { name = name }):get()
					end
					local other = appearance.provider:get()
					if other then
						return ui.t("plugins.settings.wallpaper.instead_of", { name = name, other = other.name }):get()
					end
					return ui.t("plugins.settings.wallpaper.off", { name = name }):get()
				end,
				control = ds.Switch {
					value = on,
					label = label_of(s),
					on_change = function(v: boolean)
						appearance.use_plugin(plugin, v)
					end,
				},
			}
		),
		ui.show(function(): boolean
			return on:get()
		end, ui.column {
			w_full = true,
			ds.Settings.Rule(),
			search.anchor(
				`{s.id}.effect`,
				ds.Settings.Row {
					label = ui.t("settings.appearance.effect.label"),
					description = function(): string
						return ui.t(`settings.appearance.effect.{appearance.plugin_effect:get()}.description`):get()
					end,
					control = wallpaper.EffectSelect(appearance.plugin_effect),
				}
			),
		}),
	}
end)

-- A plugin's page.
local function view(plugin: string): Node
	local function name(): string
		local e = M.of(plugin)
		return if e then e.name else plugin
	end
	-- Its item as the page lists it (who made it).
	local function item(): any
		for _, p in require("@app/screens/plugins/state").now:get().list or {} do
			if p.id == plugin then
				return p
			end
		end
		return nil
	end
	return ds.Settings.Page {
		title = name,
		-- Unverified, at the top right, for one whose maker Zeron can't
		-- vouch for (as its row says).
		trail = ui.show(function(): boolean
			return words.unverified(item())
		end, ds.Badge { ui.t("plugins.words.unverified"), tone = "warning", class = "plugin-unverified" }),
		trail_top = true,
		description = function(): string
			local e = M.of(plugin)
			if e and type(e.description) == "string" and e.description ~= "" then
				return e.description
			end
			return ui.t("plugins.settings.description", { name = name() }):get()
		end,
		ui.presence(function(): string
			return shape(M.of(plugin))
		end, function(_key: string): Node
			local e = M.of(plugin)
			if not e then
				return ui.div {}
			end
			local out: { Node } = {}
			for _, run in sections(e.settings) do
				local rows: { Node } = {}
				for _, s in run.settings do
					if s.kind == "wallpaper" then
						table.insert(rows, WallpaperRows { plugin = plugin, setting = s, name = e.name })
						continue
					end
					local permission = if s.kind == "permission" then permissions.KNOWN[s.permission or ""] else nil
					table.insert(
						rows,
						search.anchor(
							s.id,
							ds.Settings.Row {
								-- (A permission's words Zeron's, read as they're drawn.)
								label = if permission
									then function(): string
										return label_of(s)
									end
									else label_of(s),
								description = if permission
									then function(): string
										return permission.description(e.name)
									end
									else description_of(plugin, s),
								control = ui.row {
									gap = "$space.2",
									items = "center",
									if permission then nil else status_badge(plugin, s.id),
									Control { plugin = plugin, setting = s, name = e.name },
								},
							}
						)
					)
				end
				-- (Zeron's note under permission rows: whose permissions they are.)
				local asks = false
				for _, s in run.settings do
					asks = asks or s.kind == "permission"
				end
				table.insert(out, ds.Settings.Section {
					title = run.title,
					description = if asks then permissions.note() else nil,
					ds.Settings.Group(rows :: any),
				})
			end
			return ui.column { w_full = true, gap = "$space.6", table.unpack(out) }
		end, { w_full = true }),
	}
end

-- The pages, after Zeron's own, under "Plugins". (A template whose
-- Settings shell predates pages that come and go lists none.)
-- A plugin's own icon (the SVG the host read from it), if it ships one.
local function icon_of(plugin: string): string?
	local list = require("@app/screens/plugins/state").now:get().list or {}
	for _, p in list do
		if p.id == plugin and type(p.icon_svg) == "string" and p.icon_svg ~= "" then
			return p.icon_svg
		end
	end
	return nil
end
local function pages(): { shell.Page }
	local out: { shell.Page } = {}
	for _, e in M.list:get() do
		local plugin = e.plugin
		table.insert(out, {
			id = M.page_id(plugin),
			title = e.name,
			icon = "widget",
			tile = { label = e.name, hue = words.hue(plugin), svg = icon_of(plugin) },
			group = "Plugins",
			private = true,
			view = function(): Node
				return view(plugin)
			end,
		})
	end
	return out
end
local extra = (shell :: any).extra
if extra then
	extra("plugins", pages)
end

-- In Settings search, as they come and go.
local indexed: { [string]: boolean } = {}
ui.effect(function()
	local now: { [string]: boolean } = {}
	for _, e in M.list:get() do
		local page = M.page_id(e.plugin)
		now[page] = true
		local rows: { search.Row } = {}
		for _, s in e.settings do
			local terms = if s.kind == "permission"
				then ui.t("plugins.settings.permission_terms")
				elseif s.kind == "wallpaper" then ui.t("plugins.settings.wallpaper.terms")
				else s.description
			table.insert(rows, { key = s.id, title = label_of(s), section = s.section, terms = terms })
		end
		search.index(page, e.name, ui.t("plugins.settings.terms"), rows)
	end
	local unindex = (search :: any).unindex
	for page in indexed do
		if not now[page] and unindex then
			unindex(page)
		end
	end
	indexed = now
end)

return M

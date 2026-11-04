--!strict
-- Settings → Plugins: the plugins on one of your computers, this one first.
-- The gallery's Plugins example (demos/example_plugins.luau) with real
-- data: the same page, search, tabs and rows. Its tabs are what's installed
-- and what's yours (there's no catalog yet). Each row is its plugin's as the
-- list changes, and what's at its end swaps only when what it can do
-- does: a switch once you've allowed it; Review while it waits for you;
-- the folder when it can't load.
--
-- Plugins are per device (what each runs, what each allowed), so the page
-- has Providers' device choice at its title's end (screens/settings/
-- devices `picker`): another device's plugins are listed, allowed, turned
-- on or off, installed, updated and removed there, the engine carrying
-- each call (`targetDeviceId`, src/plugins/remote.rs). What's this
-- device's alone isn't offered there: a plugin's settings (its windows
-- here read them), its folder, and installing from a file on this one. A
-- device that's away says so in place of its list.
--
-- Allowing happens in a window of its own (screens/plugins/consent),
-- never here: this page runs beside plugins and your template, which
-- must not be able to allow anything themselves.
--
-- Someone else's plugins come in by Install… (a link: a package, a GitHub
-- release, a git repository; or, on this device, a .zeron-plugin file):
-- the engine checks the signature, then it waits for the consent window
-- like any other. Each one's source is checked for a newer version as the
-- page opens on a device (the engine checks every few hours too); a row
-- with one says what it changes in what the plugin asks and has Update.

local ds = require("@ds")
local sessions = require("@account/sessions")
local state = require("@app/screens/plugins/state")
local words = require("@app/screens/plugins/words")
local plugin_settings = require("@app/screens/plugins/settings")
local devices_page = require("@app/screens/settings/devices")
local visits = require("@app/screens/settings/visits")

local M = {}

local tab = ui.state("plugins_tab", "installed")
local query = ui.state("plugins_query", "")
-- The plugin Remove was asked of, and its device (nil: this one), kept as
-- the dialog opened.
local removing = ui.state("plugins_removing", nil :: string?)
local removing_on = ui.state("plugins_removing_on", nil :: string?)
local removing_open = ui.state("plugins_removing_open", false)
local naming = ui.state("plugins_naming", false)
local new_name = ui.state("plugins_new_name", "")
local installing = ui.state("plugins_installing", false)
local install_link = ui.state("plugins_install_link", "")
-- An install on its way: the dialog stays open, its Install spinning,
-- until it's done (closed) or fails (kept, to try again).
local install_busy = ui.state("plugins_install_busy", false)
local updating = ui.state("plugins_updating", nil :: string?)
-- When each device last had its sources checked (by its id; "" this one):
-- at most every ten minutes, so reloads, quick visits and switching back
-- and forth don't.
local checked_at = ui.state("plugins_checked_at", {} :: { [string]: number })

-- The device shown: another's id, or nil (this one).
local function device(): string?
	return state.device()
end

local function shown_device(): sessions.Device?
	local id = device()
	for _, d in sessions.now:get().devices or {} do
		if (id and d.id == id) or (not id and d.here) then
			return d
		end
	end
	return nil
end

-- Its name, for a sentence (in the locale shown: read where it's drawn).
local function device_name(): string
	local d = shown_device()
	if d == nil or d.here then
		return ui.t("settings.providers.this_mac"):get()
	end
	return d.name
end

-- Another device shown that's away (its presence says so, or it couldn't
-- be reached).
local function away(): boolean
	if not device() then
		return false
	end
	local d = shown_device()
	return state.remote:get().state == "offline" or (d ~= nil and d.online == false)
end

-- (As a row is built: read once, not followed. Rows are keyed by device.)
local function device_now(): string?
	local d = state.remote:peek().device
	return if type(d) == "string" and d ~= "" then d else nil
end

local function choose(id: string)
	local here = false
	for _, d in sessions.now:peek().devices or {} do
		if d.id == id then
			here = d.here
		end
	end
	state.target(if here then nil else id)
end

-- (Its name read as the dialog opens, in the locale then.)
local function package_filter(): { any }
	return { { ui.t("plugins.page.package_filter"):get(), { "zeron-plugin" } } }
end

-- What went wrong asking the host, said once: a device that couldn't be
-- reached says so by its name.
local function failed(title: string | Message): (err: any) -> ()
	local where = device_name()
	return function(err: any)
		local message = if type(err) == "table" and err.message then tostring(err.message) else nil
		ds.toast.show {
			title = title,
			body = if message == "offline" then ui.t("plugins.page.cant_reach", { device = where }) else message,
			tone = "warning",
		}
	end
end

-- An install or update's outcome: someone else's waiting says so with its
-- own notice (screens/plugins/waiting); otherwise a word.
-- (`plain` its key without a version, `versioned` with one.)
local function settled(plain: string, versioned: string): (done: any) -> ()
	return function(done: any)
		if type(done) ~= "table" or type(done.plugin) ~= "table" then
			return
		end
		if done.waiting then
			return
		end
		local name = done.plugin.name or done.plugin.id
		local version = if type(done.plugin.version) == "string" and done.plugin.version ~= "" then done.plugin.version else nil
		ds.toast.show {
			title = if version then ui.t(versioned, { name = name, version = version }) else ui.t(plain, { name = name }),
			tone = "success",
		}
	end
end

local function install(from: { path: string?, url: string?, device: string? })
	local p: any = state.install(from)
	if p == nil then
		installing:set(false)
		return
	end
	install_busy:set(true)
	p:after(function(done: any)
		install_busy:set(false)
		installing:set(false)
		install_link:set("")
		settled("plugins.page.installed_toast", "plugins.page.installed_toast_version")(done)
	end, function(err: any)
		install_busy:set(false)
		failed(ui.t("plugins.page.couldnt_install"))(err)
	end)
end

-- Install's file: a .zeron-plugin (this device's alone), installed with
-- the dialog open as a link is.
local function choose_file()
	if install_busy:peek() then
		return
	end
	ui.task(function()
		local files = ui.open_files({ title = ui.t("plugins.page.install_title"):get(), filters = package_filter() }):await()
		local file = files and files[1]
		if file then
			install { path = file.path }
		end
	end)
end

local function update(id: string, name: string)
	updating:set(id)
	local p: any = state.update(id, device())
	if p == nil then
		updating:set(nil)
		return
	end
	p:after(function(done: any)
		updating:set(nil)
		settled("plugins.page.updated_toast", "plugins.page.updated_toast_version")(done)
	end, function(err: any)
		updating:set(nil)
		failed(ui.t("plugins.page.couldnt_update", { name = name }))(err)
	end)
end

-- Each device's sources checked as it's shown, at most every ten minutes.
local function check_updates()
	local at = checked_at:peek()
	local on = device_now()
	local key = on or ""
	if os.time() - (at[key] or 0) <= 600 then
		return
	end
	local next = table.clone(at)
	next[key] = os.time()
	checked_at:set(next)
	local p: any = state.check_updates(on)
	if p ~= nil then
		p:after(function(_: any) end, function(_: any) end)
	end
end

local function matches(p: state.Item, q: string): boolean
	if q == "" then
		return true
	end
	local hay = string.lower(`{p.name} {p.description or ""} {words.author(p)}`)
	return string.find(hay, q, 1, true) ~= nil
end

-- The plugins the tab holds that the search finds, on the device shown.
local found = ui.computed(function(): { state.Item }
	local q = string.lower(query:get())
	local mine = tab:get() == "yours"
	local out: { state.Item } = {}
	for _, p in state.shown:get() do
		if (not mine or p.own or p.author.kind == "you") and matches(p, q) then
			table.insert(out, p)
		end
	end
	return out
end)

-- (Another device's while its list isn't known, away or still coming: a
-- dash, never a 0 that says it has none.)
local function count(which: string): () -> number
	return function(): number
		if device() and (away() or state.remote:get().state ~= "ready") then
			return "–" :: any
		end
		local n = 0
		for _, p in state.shown:get() do
			if which == "installed" or p.own or p.author.kind == "you" then
				n += 1
			end
		end
		return n
	end
end

-- What a row ends with, by what the plugin can do now.
local function kind(p: state.Item): string
	if p.failed then
		return "broken"
	elseif p.status == "off" and #(p.ask or {}) > 0 then
		-- Declined ("Not now"): reviewed again from here.
		return "review"
	elseif p.status == "on" or p.status == "off" then
		return "switch"
	elseif p.status == "pending" or p.status == "tampered" then
		return "review"
	end
	return "broken"
end

-- How long a switch's knob takes to land (ds/switch: a 180 ms fill, its
-- spring a little longer).
local SETTLE_MS = 260

-- (A row is built for one device: the rows are keyed by it, so a switch of
-- device builds them again. `on` is that device, nil for this one.)
local Control = ui.component(function(props: { now: State<any>, on: string? }): Node
	local now, on_device = props.now, props.on
	local on = ui.use_state(now:peek().status == "on")
	ui.effect(function()
		local want = now:get().status == "on"
		if on:peek() ~= want then
			on:set(want)
		end
	end)
	local function remove()
		removing:set(now:peek().id)
		removing_on:set(on_device)
		removing_open:set(true)
	end
	return ui.row {
		gap = "$space.2",
		items = "center",
		-- Its settings, when it's on here and has some: their page. (Its room
		-- kept on every row, empty where there are none, so the versions line
		-- up. Another device's plugins' settings are that device's.)
		ui.div {
			w = function(): number
				return (ui.theme.get("zeron.icon_button.sm") :: number?) or 22
			end,
			shrink = 0,
			ui.show(function(): boolean
				return on_device == nil and plugin_settings.has(now:get().id)
			end, ds.IconButton {
			icon = "settings-minimalistic",
			label = function(): string
				return ui.t("plugins.page.settings_of", { name = now:get().name }):get()
			end,
			size = "sm",
			tone = "quiet",
			hook = false,
			on_click = function()
				plugin_settings.open(now:peek().id)
			end,
		}),
		},
		ui.switch(function(): string
			return kind(now:get())
		end, {
			switch = ds.Switch {
				value = on,
				label = function(): string
					return now:get().name
				end,
				-- (Asked once the knob has landed: turning a plugin on or off
				-- reloads the window's scripts to load or drop it, which
				-- rebuilds this switch already in place, cutting its slide.)
				on_change = function(value: boolean)
					local id, name = now:peek().id, now:peek().name
					ui.after(SETTLE_MS, function()
						if on:peek() ~= value then
							return
						end
						local p: any = state.set_off(id, not value, on_device)
						if p ~= nil then
							-- (Not done, as on a device out of reach: back as it is.)
							p:after(function(_: any) end, function(err: any)
								on:set(now:peek().status == "on")
								failed(ui.t("plugins.page.couldnt_switch", { name = name }))(err)
							end)
						end
					end)
				end,
			},
			review = ds.Button {
				tone = "accent",
				size = "sm",
				on_click = function()
					state.review(now:peek().id, on_device, if on_device then device_name() else nil)
				end,
				ui.t("plugins.page.review"),
			},
			-- (Its folder is on its device: here, shown; elsewhere, its words
			-- say what's wrong.)
			broken = if on_device == nil
				then ds.Button {
					tone = "secondary",
					size = "sm",
					on_click = function()
						state.open_folder(now:peek().id)
					end,
					ui.t("plugins.page.show_folder"),
				}
				else ui.div {},
		}),
		-- (Zeron's own are turned off, never removed: their button is there,
		-- off, so every row's controls line up.)
		ds.IconButton {
			icon = "trash-bin-minimalistic",
			label = function(): string
				local name = now:get().name
				return if now:get().builtin
					then ui.t("plugins.page.builtin", { name = name }):get()
					else ui.t("plugins.page.remove_named", { name = name }):get()
			end,
			size = "sm",
			tone = "quiet",
			hook = false,
			disabled = function(): boolean
				return now:get().builtin == true
			end,
			on_click = function()
				if not now:peek().builtin then
					remove()
				end
			end,
		},
	}
end)

-- Update, at the end of the line saying what it changes: link-like (the
-- markdown link's underline, the accent under the pointer), so the row's
-- control and its version stay where every other row has them.
local UpdateLink = ui.component(function(props: { now: State<any> }): Node
	local now = props.now
	local function busy(): boolean
		return updating:get() == now:get().id
	end
	return ui.show(function(): boolean
		local it: any = now:get()
		return type(it.update) == "table" and it.update.refused == nil and not it.builtin and kind(it) ~= "broken"
	end, ui.row {
		role = "button",
		label = ui.t("plugins.page.update"),
		focusable = function(): boolean
			return not busy()
		end,
		disabled = busy,
		shrink = 0,
		cursor = "pointer",
		color = "$text.primary",
		hover = { color = "$accent" },
		on_click = function()
			if not busy() then
				update(now:peek().id, now:peek().name)
			end
		end,
		ui.text {
			function(): string
				return if busy() then ui.t("plugins.page.updating"):get() else ui.t("plugins.page.update"):get()
			end,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			weight = "$font.weight.medium",
			underline = true,
			underline_offset = 4,
		},
	})
end)

local function row(p: state.Item, _i: number, now: State<any>): Node
	return ds.Settings.Row {
		label = function(): string
			return now:get().name
		end,
		description = function(): string
			local it = now:get()
			-- (The host's words: where it was placed and couldn't show, or
			-- what this template doesn't have.)
			local problem = (it :: any).problem
			if type(problem) == "string" and problem ~= "" and it.status == "on" then
				return problem
			end
			return words.about(it)
		end,
		-- Who made it, on its own line.
		byline = function(): string
			return words.author(now:get())
		end,
		-- (Its icon from its device: the listing carries it.)
		lead = words.tile(p, 32),
		-- (Zeron's own: what it's allowed, wrapped on a line of its own.)
		-- (Someone else's: a newer version found where it came from.)
		detail = function(): string?
			local it = now:get()
			return words.grants(it) or words.update(it)
		end,
		detail_action = UpdateLink { now = now },
		truncate = true,
		-- Its version, centred on the row beside its controls; before it,
		-- Unverified for one whose maker Zeron can't vouch for.
		control = ui.row {
			gap = "$space.3",
			items = "center",
			ui.show(function(): boolean
				return words.unverified(now:get())
			end, ds.Badge { ui.t("plugins.words.unverified"), tone = "warning", class = "plugin-unverified" }),
			ds.Badge {
				function(): string
					return now:get().version
				end,
				tone = "neutral",
				tabular = true,
			},
			Control { now = now, on = device_now() },
		},
	}
end

-- What a tab says with no plugins at all: here, the folder they go in;
-- on another device, a link to install one from.
local function none(which: string): Node
	local title = if which == "yours" then ui.t("plugins.page.none_yours") else ui.t("plugins.page.none")
	return ui.switch(function(): string
		return if device() then "elsewhere" else "here"
	end, {
		here = ds.EmptyState {
			icon = "widget",
			title = title,
			body = ui.t("plugins.page.none_body"),
			size = "sm",
			action = {
				label = ui.t("plugins.page.show_plugins_folder"),
				on_click = function()
					state.open_folder(nil)
				end,
			},
		},
		elsewhere = ds.EmptyState {
			icon = "widget",
			title = title,
			body = ui.t("plugins.page.none_body_device"),
			size = "sm",
			action = {
				label = ui.t("plugins.page.install_link"),
				on_click = function()
					installing:set(true)
				end,
			},
		},
	})
end

-- Another device that's away: its name, and a way to look again.
local function away_notice(): Node
	return ds.Callout {
		tone = "neutral",
		icon = "wifi-off",
		title = function(): string
			return ui.t("plugins.page.device_offline_title", { device = device_name() }):get()
		end,
		body = ui.t("plugins.page.device_offline_body"),
		action = {
			label = ui.t("plugins.page.try_again"),
			on_click = function()
				local d = device()
				if d then
					state.target(d)
				end
			end,
		},
	}
end

local function dialogs(): { Node }
	return {
		ds.Dialog {
			open = removing_open,
			title = ui.t("plugins.page.remove_title"),
			body = function(): string
				return if removing_on:get()
					then ui.t("plugins.page.remove_body_device", { device = device_name() }):get()
					else ui.t("plugins.page.remove_body"):get()
			end,
			tone = "danger",
			confirm = {
				label = ui.t("common.remove"),
				on_click = function()
					local id, on = removing:peek(), removing_on:peek()
					removing:set(nil)
					if id then
						local p: any = state.remove(id, on)
						if p ~= nil then
							p:after(function(_: any) end, failed(ui.t("plugins.page.couldnt_remove")))
						end
					end
				end,
			},
		},
		ds.Dialog {
			open = naming,
			title = ui.t("plugins.page.new_title"),
			body = ui.t("plugins.page.new_body"),
			size = "sm",
			confirm = {
				label = ui.t("plugins.page.make_it"),
				on_click = function()
					local name = string.match(new_name:peek(), "^%s*(.-)%s*$") or ""
					if name ~= "" then
						state.new_plugin(name)
						tab:set("yours")
					end
				end,
			},
			ds.TextField {
				value = new_name,
				label = ui.t("plugins.page.name"),
				placeholder = ui.t("plugins.page.name_placeholder"),
				select_on_focus = true,
				w_full = true,
			},
		},
		-- Install…: a link on any device; a file only here (a file on this
		-- device isn't on another, so there it says so instead).
		ds.Dialog {
			open = installing,
			title = function(): string
				return if device()
					then ui.t("plugins.page.install_dialog_title_device", { device = device_name() }):get()
					else ui.t("plugins.page.install_dialog_title"):get()
			end,
			body = function(): string
				return if device() then ui.t("plugins.page.install_dialog_body_device"):get() else ui.t("plugins.page.install_dialog_body"):get()
			end,
			size = "sm",
			confirm = {
				label = ui.t("plugins.page.install"),
				keep_open = true,
				busy = function(): boolean
					return install_busy:get()
				end,
				on_click = function()
					local link = string.match(install_link:peek(), "^%s*(.-)%s*$") or ""
					if link ~= "" then
						install { url = link, device = device() }
					end
				end,
			},
			ds.TextField {
				value = install_link,
				label = ui.t("plugins.page.link"),
				placeholder = "github.com/owner/plugin",
				select_on_focus = true,
				w_full = true,
			},
			ui.show(function(): boolean
				return device() == nil
			end, ds.Button {
				id = "plugins-choose-file",
				tone = "secondary",
				size = "sm",
				disabled = function(): boolean
					return install_busy:get()
				end,
				on_click = choose_file,
				ui.t("plugins.page.choose_file"),
			}),
			ui.show(function(): boolean
				return device() ~= nil
			end, ui.text {
				id = "plugins-file-elsewhere",
				function(): string
					return ui.t("plugins.page.file_here_only", { device = device_name() }):get()
				end,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$text.secondary",
				wrap = true,
			}),
		},
	}
end

-- The page: ds.Settings.Catalog's search, tabs, card and states (the
-- Templates page is the same), with this page's device choice, rows, empty
-- state, offline states and dialogs.
function M.View(): Node
	-- Once a visit: another device shown is looked at again (its list as
	-- it is now; it may have been away).
	visits.once("plugins", function()
		local d = device()
		if d then
			state.target(d)
		end
	end)
	-- As a device is shown: its plugins' sources, for a newer version.
	local shown_id = ui.computed(function(): string
		return device() or ""
	end)
	ui.effect(function()
		shown_id:get()
		check_updates()
	end)
	-- A device shown that was away coming back: looked at again.
	local was_online: boolean? = nil
	ui.effect(function()
		local d = shown_device()
		local id = device()
		local online = d ~= nil and d.online ~= false
		if id and online and was_online == false and state.remote:peek().state == "offline" then
			state.target(id)
		end
		was_online = if id then online else nil
	end)
	return ds.Settings.Catalog {
		title = ui.t("plugins.page.title"),
		description = ui.t("plugins.page.description"),
		trail = devices_page.picker(device, choose),
		-- (At the top right, on the title's line, as Providers' is.)
		trail_top = true,
		search = { value = query, placeholder = ui.t("plugins.page.search") },
		tabs = {
			value = tab,
			label = ui.t("plugins.page.tabs"),
			options = {
				{ value = "installed", label = ui.t("plugins.page.installed"), count = count("installed") },
				{ value = "yours", label = ui.t("plugins.page.yours"), count = count("yours") },
			},
		},
		-- (No New for now: your own plugins arrive by an agent or the
		-- folder. Install… brings in someone else's, on the device shown.)
		import = {
			label = ui.t("plugins.page.install_more"),
			on_click = function()
				installing:set(true)
			end,
		},
		status = function(): string
			if device() then
				if away() then
					return "away"
				end
				return if state.remote:get().state == "ready" then "ready" else "loading"
			end
			return state.now:get().state
		end,
		states = {
			offline = function(): Node
				return ds.Callout {
					tone = "warning",
					title = ui.t("plugins.page.offline_title"),
					body = ui.t("plugins.page.offline_body"),
				}
			end,
			-- (The account's setup isn't finished: plugins load once it is,
			-- src/plugins `wait_for_setup`.)
			setup = function(): Node
				return ds.Callout {
					tone = "info",
					title = ui.t("plugins.page.setup_title"),
					body = ui.t("plugins.page.setup_body"),
				}
			end,
			away = away_notice,
		},
		found = found,
		row = row,
		-- (By device and id: another device's rows are its own.)
		key = function(p: state.Item): string
			return `{device() or ""}/{p.id}`
		end,
		none = none,
		overlays = dialogs(),
	}
end

return M

-- Settings → Voice: dictation, as the shipping app's page has it (crates/ui
-- dictation/model.rs): turning it on downloads the speech model the first time,
-- then the microphone, the shortcut, and the model to remove.

local host = require("@app/screens/settings/host")
local ds = require("@ds")
local toast = require("@ds/toast")
local Shortcut = require("@ds/shortcut_recorder")
local prefs = require("@app/screens/settings/prefs")
local search = require("@app/screens/settings/search")
local visits = require("@app/screens/settings/visits")
local keys = require("@app/screens/shell/keys")
local dictation = require("@app/screens/chat/dictation")

local M = {}

M.ID = "voice"
M.TITLE = "Voice"
local SHORTCUT = "shell.dictate"

search.index(M.ID, ui.t("settings.voice.title"), ui.t("settings.voice.terms"), {
	{ key = "dictation", title = ui.t("settings.voice.dictation.label") },
	{ key = "microphone", title = ui.t("settings.voice.microphone") },
	{ key = "shortcut", title = ui.t("settings.voice.shortcut.label") },
	{ key = "model", title = ui.t("settings.voice.model.title") },
})

-- The models on offer, as the host lists them: { id, name, size,
-- languages, installed }, the default first.
local models = ui.state("settings_voice_models", {} :: { any })
-- The chosen one (its first when the saved one isn't offered).
local function chosen(): any?
	local list, want = models:get(), prefs.dictation_model:get()
	for _, m in list do
		if m.id == want then return m end
	end
	return list[1]
end
local installed = ui.computed(function(): boolean
	local m = chosen()
	return m ~= nil and m.installed
end)
local size = ui.computed(function(): number
	local m = chosen()
	return if m then m.size else 0
end)
local inputs = ui.state("settings_voice_inputs", {} :: { any })
local on = ui.state("settings_voice_on", false)

local function refresh()
	host.call("dictation.call", { op = "status" }):after(function(s: any)
		models:set(s.models or {})
	end, function() end)
	host.call("dictation.call", { op = "inputs" }):after(function(list: any)
		inputs:set(list)
	end, function() end)
end
local function refused(err: any)
	toast.show {
		title = ui.t("settings.voice.couldnt"):get(),
		body = if type(err) == "table" and err.message then tostring(err.message) else nil,
		tone = "warning",
	}
end
local function downloading(): boolean
	return dictation.state:get().phase == "downloading"
end
local function mb(bytes: number): string
	return string.format("%.0f", bytes / 1e6)
end
-- A model's size: "740 MB", "1.8 GB".
local function weight(bytes: number): string
	return if bytes >= 1e9
		then ui.t("settings.voice.model.gb", { n = string.format("%.1f", bytes / 1e9) }):get()
		else ui.t("settings.voice.model.mb", { n = mb(bytes) }):get()
end

-- The switch says what's so: on while downloading, or on with the model here.
ui.effect(function()
	local now = downloading() or (prefs.dictation:get() and installed:get())
	if on:peek() ~= now then
		on:set(now)
	end
end)
local function mark(id: string, here: boolean)
	local list = table.clone(models:peek())
	for i, m in list do
		if m.id == id then
			list[i] = table.clone(m)
			list[i].installed = here
		end
	end
	models:set(list)
end
-- A download ending: that model chosen and on when it arrived, and why not
-- when it didn't.
local fetching = ui.state("settings_voice_fetching", "")
ui.effect(function()
	local s = dictation.state:get()
	local id = fetching:peek()
	if s.phase == "downloading" or id == "" then
		return
	end
	fetching:set("")
	if s.phase == "ready" then
		ui.batch(function()
			mark(id, true)
			prefs.dictation_model:set(id)
			prefs.dictation:set(true)
		end)
	else
		on:set(false)
		if s.phase == "download_failed" then
			refused({ message = s.error })
		end
	end
end)
local function download(id: string)
	fetching:set(id)
	host.call("dictation.call", { op = "download", model = id }):after(function() end, function(err: any)
		fetching:set("")
		on:set(false)
		refused(err)
	end)
end
local function flip(want: boolean)
	local m = chosen()
	if downloading() then
		host.call("dictation.call", { op = "cancel" }):after(function() end, refused)
	elseif installed:peek() or not want then
		prefs.dictation:set(want)
	elseif m then
		download(m.id)
	end
end
-- Removing the chosen model turns dictation off, as with no model at all.
local function remove(id: string)
	host.call("dictation.call", { op = "remove", model = id }):after(function()
		ui.batch(function()
			mark(id, false)
			local m = chosen()
			if m == nil or m.id == id then
				prefs.dictation:set(false)
			end
		end)
	end, refused)
end

local function row(key: string, props: any): Node
	return search.anchor(key, ds.Settings.Row(props))
end
local function microphones(): { any }
	local saved = prefs.dictation_input:get()
	local options = { { value = "system", label = ui.t("settings.voice.system_default"):get() } }
	local listed = saved == "system"
	for _, d in inputs:get() do
		table.insert(options, { value = d.id, label = d.name })
		listed = listed or d.id == saved
	end
	-- (Unplugged, it stays chosen and the system's records: back when it's back.)
	if not listed then
		table.insert(options, { value = saved, label = ui.t("settings.voice.disconnected"):get() })
	end
	return options
end

-- A model's line: its size, how many languages it speaks, and whether it's
-- slow here (on the CPU).
local function about(m: any): string
	return ui.t(if m.slow then "settings.voice.model.about_slow" else "settings.voice.model.about", { size = weight(m.size), n = #(m.languages or {}) }):get()
end
-- Download or Remove `m` (a function: the model as it is now).
local function model_action(m: () -> any?): Node
	local function here(): boolean
		local now = m()
		return now ~= nil and now.installed
	end
	return ui.row {
		ui.show(here, ds.Button {
			ui.t("settings.voice.model.remove"), tone = "secondary", size = "sm",
			on_click = function() remove(m().id) end,
		}),
		ui.show(function(): boolean return m() ~= nil and not here() end, ds.Button {
			ui.t("settings.voice.model.download"), tone = "secondary", size = "sm", disabled = downloading,
			on_click = function() download(m().id) end,
		}),
	}
end

-- How long the model in use stays ready after a dictation, what that holds,
-- and what the wait is otherwise.
local function holds(): string
	local m = chosen()
	return if m and m.memory then weight(m.memory) else ""
end
local function name(): string
	local m = chosen()
	return if m then m.name else ""
end
-- Seconds (prefs.dictation_keep): 0 released at once, -1 while Zeron is open.
local keep_choices = {
	{ value = 0, key = "settings.voice.keep.instant" },
	{ value = 15, key = "settings.voice.keep.s15" },
	{ value = 300, key = "settings.voice.keep.m5" },
	{ value = 900, key = "settings.voice.keep.m15" },
	{ value = 3600, key = "settings.voice.keep.h1" },
	{ value = -1, key = "settings.voice.keep.always" },
}
local function keep_row(): Node
	return ui.show(installed, ds.Settings.Group {
		ds.Settings.Row {
			label = ui.t("settings.voice.keep.label"),
			description = function(): string
				return ui.t("settings.voice.keep.description", { name = name(), memory = holds() }):get()
			end,
			control = ds.Select {
				value = prefs.dictation_keep,
				name = ui.t("settings.voice.keep.label"),
				options = function(): { any }
					local list = {}
					for _, c in keep_choices do
						table.insert(list, { value = c.value, label = ui.t(c.key):get() })
					end
					return list
				end,
				w = 200,
			},
		},
	})
end

-- A row a model, the one in use marked.
local function model_rows(): Node
	return search.anchor("model", ds.Settings.Section {
		title = ui.t("settings.voice.model.title"),
		ds.Settings.Each(models, function(_, _, now: any): Node
			local function m(): any return now:get() end
			local function in_use(): boolean
				local active = chosen()
				return active ~= nil and active.id == m().id and m().installed
			end
			return ds.Settings.Row {
				label = function(): string return m().name end,
				badge = ui.show(in_use, ds.Badge { ui.t("settings.voice.model.in_use"), size = "sm" }),
				description = function(): string return about(m()) end,
				control = ui.row {
					gap = "$space.2",
					items = "center",
					ui.show(function(): boolean return m().installed and not in_use() end, ds.Button {
						ui.t("settings.voice.model.use"), tone = "secondary", size = "sm",
						on_click = function() prefs.dictation_model:set(m().id) end,
					}),
					model_action(m),
				},
			}
		end, { key = function(m: any): string return m.id end }),
		keep_row(),
	})
end

function M.view(): Node
	visits.once(M.ID, refresh)
	return ds.Settings.Page {
		title = ui.t("settings.voice.title"),
		description = ui.t("settings.voice.description"),
		ds.Settings.Section {
			ds.Settings.Group {
				row("dictation", {
					label = ui.t("settings.voice.dictation.label"),
					description = function(): string
						local s = dictation.state:get()
						if s.phase == "downloading" then
							return ui.t("settings.voice.dictation.downloading", { done = weight(s.bytes or 0), total = weight(s.total or 0) }):get()
						elseif not installed:get() then
							return ui.t("settings.voice.dictation.needs_download", { total = weight(size:get()) }):get()
						end
						return ui.t("settings.voice.dictation.description"):get()
					end,
					control = ds.Switch { value = on, label = ui.t("settings.voice.dictation.label"), on_change = flip },
				}),
				-- (Stays put while it's off, dimmed and out of reach.)
				search.anchor(
					"microphone",
					ui.div {
						w_full = true,
						opacity = function()
							return if on:get() then 1 else "$zeron.disabled"
						end,
						disabled = function()
							return not on:get()
						end,
						ds.Settings.Row {
							label = ui.t("settings.voice.microphone"),
							control = ds.Select { value = prefs.dictation_input, options = microphones, name = ui.t("settings.voice.microphone"), w = 200 },
						},
					}
				),
				row("shortcut", {
					label = ui.t("settings.voice.shortcut.label"),
					description = ui.t("settings.voice.shortcut.description"),
					control = Shortcut.Recorder {
						value = function(): string
							return keys.combo(SHORTCUT)
						end,
						label = ui.t("settings.voice.shortcut.label") :: any,
						on_change = function(combo: string)
							local owner = keys.owner(combo, SHORTCUT)
							local taken = if owner then keys.get(owner) else nil
							if owner then
								refused({ message = ui.t("settings.shortcuts.taken", { keys = Shortcut.shown(combo), name = if taken then keys.title(taken) else owner }):get() })
								return
							end
							keys.bind(SHORTCUT, combo)
						end,
					},
				}),
			},
		},
		model_rows(),
	}
end

return M

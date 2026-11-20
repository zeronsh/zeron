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
	{ key = "model", title = ui.t("settings.voice.model.label") },
})

local installed = ui.state("settings_voice_installed", false)
local size = ui.state("settings_voice_size", 0)
local inputs = ui.state("settings_voice_inputs", {} :: { any })
local on = ui.state("settings_voice_on", false)

local function refresh()
	host.call("dictation.call", { op = "status" }):after(function(s: any)
		ui.batch(function()
			installed:set(s.installed)
			size:set(s.size)
		end)
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

-- The switch says what's so: on while downloading, or on with the model here.
ui.effect(function()
	local now = downloading() or (prefs.dictation:get() and installed:get())
	if on:peek() ~= now then
		on:set(now)
	end
end)
-- A download ending: on when it arrived, and why not when it didn't.
local fetching = ui.state("settings_voice_fetching", false)
ui.effect(function()
	local s = dictation.state:get()
	if s.phase == "downloading" then
		fetching:set(true)
	elseif fetching:peek() then
		fetching:set(false)
		if s.phase == "ready" then
			installed:set(true)
			prefs.dictation:set(true)
		else
			on:set(false)
			if s.phase == "download_failed" then
				refused({ message = s.error })
			end
		end
	end
end)
local function flip(want: boolean)
	if downloading() then
		host.call("dictation.call", { op = "cancel" }):after(function() end, refused)
	elseif installed:peek() or not want then
		prefs.dictation:set(want)
	else
		host.call("dictation.call", { op = "download" }):after(function() end, function(err: any)
			on:set(false)
			refused(err)
		end)
	end
end
local function remove()
	host.call("dictation.call", { op = "remove" }):after(function()
		ui.batch(function()
			installed:set(false)
			prefs.dictation:set(false)
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
							return ui.t("settings.voice.dictation.downloading", { done = mb(s.bytes or 0), total = mb(s.total or 0) }):get()
						elseif not installed:get() then
							return ui.t("settings.voice.dictation.needs_download", { total = mb(size:get()) }):get()
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
		ui.show(function(): boolean
			return installed:get() and not downloading()
		end, ds.Settings.Section {
			ds.Settings.Group {
				row("model", {
					label = ui.t("settings.voice.model.label"),
					description = function(): string
						return ui.t("settings.voice.model.description", { total = mb(size:get()) }):get()
					end,
					control = ds.Button { ui.t("settings.voice.model.remove"), tone = "secondary", size = "sm", on_click = remove },
				}),
			},
		}),
	}
end

return M

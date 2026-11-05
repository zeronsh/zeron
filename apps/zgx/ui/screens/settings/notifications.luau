-- Settings → Notifications: the shipping app's (crates/ui
-- settings/notifications.rs). Desktop banners are the engine's to post (it
-- keeps posting them with the window closed), so their switches are its
-- file's (`notifications.get`/`.set`, src/settings.rs); the session chimes
-- are this app's (prefs.luau, played by chimes.luau). Each sub-setting
-- stays put while its parent is off, dimmed and out of reach.

local host = require("@app/screens/settings/host")
local ds = require("@ds")
local toast = require("@ds/toast")
local prefs = require("@app/screens/settings/prefs")
local search = require("@app/screens/settings/search")
local visits = require("@app/screens/settings/visits")

local M = {}

M.ID = "notifications"
M.TITLE = "Notifications"

search.index(M.ID, ui.t("settings.notifications.title"), ui.t("settings.notifications.terms"), {
	{ key = "desktop", title = ui.t("settings.notifications.desktop.label"), section = ui.t("settings.notifications.desktop_section"), terms = ui.t("settings.notifications.desktop.terms") },
	{ key = "agent_updates", title = ui.t("settings.notifications.agent_updates.label"), section = ui.t("settings.notifications.desktop_section"), terms = ui.t("settings.notifications.agent_updates.terms") },
	{ key = "background_only", title = ui.t("settings.notifications.background_only.label"), section = ui.t("settings.notifications.desktop_section"), terms = ui.t("settings.notifications.background_only.terms") },
	{ key = "sounds", title = ui.t("settings.notifications.sounds.label"), section = ui.t("settings.notifications.sounds_section"), terms = ui.t("settings.notifications.sounds.terms") },
	{ key = "sound_done", title = ui.t("settings.notifications.sound_done.label"), section = ui.t("settings.notifications.sounds_section"), terms = ui.t("settings.notifications.sound_done.terms") },
	{ key = "sound_request", title = ui.t("settings.notifications.sound_request.label"), section = ui.t("settings.notifications.sounds_section"), terms = ui.t("settings.notifications.sound_request.terms") },
	{ key = "sound_attention", title = ui.t("settings.notifications.sound_attention.label"), section = ui.t("settings.notifications.sounds_section"), terms = ui.t("settings.notifications.sound_attention.terms") },
})

-- The banners' switches, as the engine's file has them (on, unread).
local desktop = ui.state("settings_banners", true)
local background_only = ui.state("settings_banners_background", true)
local agent_updates = ui.state("settings_banners_updates", true)
-- Whether they couldn't be read (the switches then hold still, as they'd
-- say what isn't so).
local unread = ui.state("settings_banners_unread", false)

local function load()
	host.call("notifications.get"):after(function(v: any)
		ui.batch(function()
			unread:set(false)
			desktop:set(v.desktop)
			background_only:set(v.background_only)
			agent_updates:set(v.agent_updates)
		end)
	end, function()
		unread:set(true)
	end)
end

-- A flip, written to the engine's file; back as it was if it can't be.
local function banner_switch(label: Label, value: State<boolean>, field: string): Node
	return ds.Switch {
		value = value,
		label = label,
		on_change = function(on: boolean)
			-- (Unread, the file isn't written over with a guess.)
			if unread:peek() then
				value:set(not on)
				return
			end
			host.call("notifications.set", { [field] = on }):after(function() end, function()
				value:set(not on)
				toast.show { title = ui.t("settings.engine.couldnt_save"):get(), body = ui.t("settings.engine.offline"):get(), tone = "danger" }
			end)
		end,
	}
end

-- A row that hangs off another: dimmed and out of reach while `parent` is
-- off.
local function under(parent: State<boolean>, key: string, props: any): Node
	return search.anchor(
		key,
		ui.div {
			w_full = true,
			opacity = function()
				return if parent:get() then 1 else "$zeron.disabled"
			end,
			disabled = function()
				return not parent:get()
			end,
			ds.Settings.Row(props),
		}
	)
end

local function row(key: string, props: any): Node
	return search.anchor(key, ds.Settings.Row(props))
end

function M.view(): Node
	visits.once("notifications", load)
	return ds.Settings.Page {
		title = ui.t("settings.notifications.title"),
		ds.Settings.Section {
			title = ui.t("settings.notifications.desktop_section"),
			ui.show(function(): boolean
				return unread:get()
			end, ds.Callout {
				tone = "warning",
				title = ui.t("settings.notifications.unread.title"),
				body = ui.t("settings.notifications.unread.body"),
				action = {
					label = ui.t("common.retry") :: any,
					on_click = function()
						visits.again("notifications", load)
					end,
				},
			}),
			ds.Settings.Group {
				row("desktop", {
					label = ui.t("settings.notifications.desktop.label"),
					description = ui.t("settings.notifications.desktop.description"),
					control = banner_switch(ui.t("settings.notifications.desktop.label"), desktop, "desktop"),
				}),
				under(desktop, "agent_updates", {
					label = ui.t("settings.notifications.agent_updates.label"),
					description = ui.t("settings.notifications.agent_updates.description"),
					control = banner_switch(ui.t("settings.notifications.agent_updates.label"), agent_updates, "agent_updates"),
				}),
				under(desktop, "background_only", {
					label = ui.t("settings.notifications.background_only.label"),
					description = ui.t("settings.notifications.background_only.description"),
					control = banner_switch(ui.t("settings.notifications.background_only.label"), background_only, "background_only"),
				}),
			},
		},
		ds.Settings.Section {
			title = ui.t("settings.notifications.sounds_section"),
			ds.Settings.Group {
				row("sounds", {
					label = ui.t("settings.notifications.sounds.label"),
					control = ds.Switch { value = prefs.sounds, label = ui.t("settings.notifications.sounds.label") },
				}),
				under(prefs.sounds, "sound_done", {
					label = ui.t("settings.notifications.sound_done.label"),
					control = ds.Switch { value = prefs.sound("done"), label = ui.t("settings.notifications.sound_done.name") },
				}),
				under(prefs.sounds, "sound_request", {
					label = ui.t("settings.notifications.sound_request.label"),
					control = ds.Switch { value = prefs.sound("request"), label = ui.t("settings.notifications.sound_request.name") },
				}),
				under(prefs.sounds, "sound_attention", {
					label = ui.t("settings.notifications.sound_attention.label"),
					control = ds.Switch { value = prefs.sound("attention"), label = ui.t("settings.notifications.sound_attention.name") },
				}),
			},
		},
	}
end

return M

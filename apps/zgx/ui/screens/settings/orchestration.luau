-- Settings → Orchestration: what the engines do with orchestrators' work.
-- For now, crew threads (chats an orchestrator started) left idle: archived
-- on their own, and after how long. The profile's, synced and read by every
-- engine (account/orchestration `settings`), not this device's alone.

local ds = require("@ds")
local search = require("@app/screens/settings/search")
local orchestration = require("@account/orchestration")

local M = {}

M.ID = "orchestration"
M.TITLE = "Orchestration"

search.index(M.ID, ui.t("settings.orchestration.title"), ui.t("settings.orchestration.terms"), {
	{ key = "auto_archive", title = ui.t("settings.orchestration.auto_archive.label"), section = ui.t("settings.orchestration.crew_section"), terms = ui.t("settings.orchestration.auto_archive.terms") },
	{ key = "idle_hours", title = ui.t("settings.orchestration.idle_hours.label"), section = ui.t("settings.orchestration.crew_section"), terms = ui.t("settings.orchestration.idle_hours.terms") },
})

local function on(): boolean
	return orchestration.auto_archive:get() == true
end

-- How long idle first, as the menu says it (a day and more in days).
local function idle_options(): { { value: number, label: string } }
	local out = {}
	for _, h in orchestration.IDLE_HOURS do
		table.insert(out, {
			value = h,
			label = if h >= 24 and h % 24 == 0
				then ui.t("settings.orchestration.idle_hours.days", { n = h // 24 }):get()
				else ui.t("settings.orchestration.idle_hours.hours", { n = h }):get(),
		})
	end
	-- (A window set elsewhere, not one of these: listed too.)
	local now = orchestration.idle_hours:get()
	if type(now) == "number" and table.find(orchestration.IDLE_HOURS, now) == nil then
		table.insert(out, { value = now, label = ui.t("settings.orchestration.idle_hours.hours", { n = now }):get() })
	end
	return out
end

function M.view(): Node
	return ds.Settings.Page {
		title = ui.t("settings.orchestration.title"),
		description = ui.t("settings.orchestration.description"),
		ds.Settings.Section {
			title = ui.t("settings.orchestration.crew_section"),
			ds.Settings.Group {
				search.anchor(
					"auto_archive",
					ds.Settings.Row {
						label = ui.t("settings.orchestration.auto_archive.label"),
						description = ui.t("settings.orchestration.auto_archive.description"),
						control = ds.Switch { value = orchestration.auto_archive, label = ui.t("settings.orchestration.auto_archive.label") },
					}
				),
				-- (Holds only while archiving is on: dimmed and out of reach
				-- while it's off.)
				search.anchor(
					"idle_hours",
					ui.div {
						w_full = true,
						opacity = function()
							return if on() then 1 else "$zeron.disabled"
						end,
						disabled = function()
							return not on()
						end,
						ds.Settings.Row {
							label = ui.t("settings.orchestration.idle_hours.label"),
							description = ui.t("settings.orchestration.idle_hours.description"),
							control = ds.Select {
								value = orchestration.idle_hours,
								name = ui.t("settings.orchestration.idle_hours.label"),
								w = 160,
								options = idle_options,
							},
						},
					}
				),
			},
		},
	}
end

return M

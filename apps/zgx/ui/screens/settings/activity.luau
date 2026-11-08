-- Settings → Activity: the Activity sidebar, and how it behaves. The view
-- itself (the sidebar's "…" turns it on too), whether working chats fold
-- into Working, what Sleep does, and how long a chat may sit idle before
-- it's archived. Everything after the view's own switch applies only while
-- it's on, so it's dimmed and out of reach while it's off. All of it is
-- this device's (screens/shell/activity).

local ds = require("@ds")
local search = require("@app/screens/settings/search")
local activity = require("@app/screens/shell/activity")

local M = {}

M.ID = "activity"
M.TITLE = "Activity"

search.index(M.ID, ui.t("settings.activity.title"), ui.t("settings.activity.terms"), {
	{ key = "view", title = ui.t("settings.activity.view.label"), terms = ui.t("settings.activity.view.terms") },
	{ key = "working", title = ui.t("settings.activity.working.label"), section = ui.t("settings.activity.list_section"), terms = ui.t("settings.activity.working.terms") },
	{ key = "sleep", title = ui.t("settings.activity.sleep.label"), section = ui.t("settings.activity.list_section"), terms = ui.t("settings.activity.sleep.terms") },
	{ key = "archive_after", title = ui.t("settings.activity.archive_after.label"), section = ui.t("settings.activity.archive_section"), terms = ui.t("settings.activity.archive_after.terms") },
})

local function row(key: string, props: any): Node
	return search.anchor(key, ds.Settings.Row(props))
end

-- A row that holds only while the view is on: dimmed and out of reach
-- while it's off.
local function while_on(key: string, props: any): Node
	return search.anchor(
		key,
		ui.div {
			w_full = true,
			opacity = function()
				return if activity.on:get() then 1 else "$zeron.disabled"
			end,
			disabled = function()
				return not activity.on:get()
			end,
			ds.Settings.Row(props),
		}
	)
end

-- How long before an idle chat is archived, as the menu says it.
local function archive_options(): { { value: number, label: string } }
	local out = {}
	for _, days in activity.ARCHIVE_AFTER do
		table.insert(out, {
			value = days,
			label = if days == 0
				then ui.t("settings.activity.archive_after.never"):get()
				else ui.t("settings.activity.archive_after.days", { n = days }):get(),
		})
	end
	return out
end

function M.view(): Node
	return ds.Settings.Page {
		title = ui.t("settings.activity.title"),
		description = ui.t("settings.activity.description"),
		ds.Settings.Section {
			ds.Settings.Group {
				row("view", {
					label = ui.t("settings.activity.view.label"),
					description = ui.t("settings.activity.view.description"),
					control = ds.Switch { value = activity.on, label = ui.t("settings.activity.view.label") },
				}),
			},
		},
		ds.Settings.Section {
			title = ui.t("settings.activity.list_section"),
			ds.Settings.Group {
				while_on("working", {
					label = ui.t("settings.activity.working.label"),
					description = ui.t("settings.activity.working.description"),
					control = ds.Switch { value = activity.working_shelf, label = ui.t("settings.activity.working.label") },
				}),
				while_on("sleep", {
					label = ui.t("settings.activity.sleep.label"),
					description = ui.t("settings.activity.sleep.description"),
				}),
			},
		},
		ds.Settings.Section {
			title = ui.t("settings.activity.archive_section"),
			ds.Settings.Group {
				while_on("archive_after", {
					label = ui.t("settings.activity.archive_after.label"),
					description = ui.t("settings.activity.archive_after.description"),
					control = ds.Select {
						value = activity.archive_after,
						name = ui.t("settings.activity.archive_after.label"),
						w = 160,
						options = archive_options,
					},
				}),
			},
		},
	}
end

return M

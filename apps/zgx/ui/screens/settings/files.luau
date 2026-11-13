-- Settings → Files: how the files pane edits and lists, the shipping app's
-- Files (crates/ui settings/files.rs), its defaults too: autosave off (and,
-- on, 900 ms after the typing stops), no wrapping, dotfiles and ignored
-- files left out of the tree. This Mac's preferences (prefs.luau), read by
-- the files pane (screens/files: its viewer and its tree) as they change.

local ds = require("@ds")
local prefs = require("@app/screens/settings/prefs")
local search = require("@app/screens/settings/search")

local M = {}

M.ID = "files"
M.TITLE = "Files"

search.index(M.ID, ui.t("settings.files.title"), ui.t("settings.files.terms"), {
	{ key = "autosave", title = ui.t("settings.files.autosave.label"), terms = ui.t("settings.files.autosave.terms") },
	{ key = "autosave_delay", title = ui.t("settings.files.autosave_delay.label"), terms = ui.t("settings.files.autosave_delay.terms") },
	{ key = "word_wrap", title = ui.t("settings.files.word_wrap.label"), terms = ui.t("settings.files.word_wrap.terms") },
	{ key = "show_all", title = ui.t("settings.files.show_all.label"), terms = ui.t("settings.files.show_all.terms") },
})

local function row(key: string, props: any): Node
	return search.anchor(key, ds.Settings.Row(props))
end

function M.view(): Node
	return ds.Settings.Page {
		title = ui.t("settings.files.title"),
		description = ui.t("settings.files.description"),
		ds.Settings.Section {
			title = ui.t("settings.files.editor"),
			ds.Settings.Group {
				-- The delay hangs off Autosave: shown while it's on (one row
				-- of the card, the rule between them coming and going with it).
				ui.column {
					w_full = true,
					row("autosave", {
						label = ui.t("settings.files.autosave.label"),
						description = ui.t("settings.files.autosave.description"),
						control = ds.Switch { value = prefs.autosave, label = ui.t("settings.files.autosave.label") },
					}),
					ui.show(function(): boolean
						return prefs.autosave:get()
					end, ui.column {
						w_full = true,
						ds.Settings.Rule(),
						row("autosave_delay", {
							label = ui.t("settings.files.autosave_delay.label"),
							description = ui.t("settings.files.autosave_delay.description"),
							control = ds.Select {
								value = prefs.autosave_delay,
								name = ui.t("settings.files.autosave_delay.label"),
								w = 128,
								options = prefs.AUTOSAVE_DELAYS,
							},
						}),
					}),
				},
				row("word_wrap", {
					label = ui.t("settings.files.word_wrap.label"),
					description = ui.t("settings.files.word_wrap.description", { key = ui.shortcut_text("alt+z") }),
					control = ds.Switch { value = prefs.word_wrap, label = ui.t("settings.files.word_wrap.label") },
				}),
			},
		},
		ds.Settings.Section {
			title = ui.t("settings.files.tree"),
			ds.Settings.Group {
				row("show_all", {
					label = ui.t("settings.files.show_all.label"),
					description = ui.t("settings.files.show_all.description"),
					control = ds.Switch { value = prefs.show_all_files, label = ui.t("settings.files.show_all.label") },
				}),
			},
		},
	}
end

return M

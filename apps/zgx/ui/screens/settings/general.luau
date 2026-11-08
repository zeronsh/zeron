-- Settings → General: how the composer and conversations behave, where
-- pull requests open, which model names threads, and this Mac's git
-- settings: the shipping app's General (crates/ui settings/shortcuts.rs's
-- page, thread_naming.rs, worktrees.rs), in the gallery's Settings parts.
--
-- The composer's and the links' are this Mac's preferences (prefs.luau),
-- read where they apply. Thread naming, commit credit and the worktree
-- folder are the engine's (`settings.engine`): asked once a visit, each
-- change shown at once and put back if the engine refuses it.

local host = require("@app/screens/settings/host")
local ds = require("@ds")
local toast = require("@ds/toast")
local prefs = require("@app/screens/settings/prefs")
local search = require("@app/screens/settings/search")
local visits = require("@app/screens/settings/visits")
local chat = require("@app/screens/chat/store")
local language = require("@app/screens/language")

local M = {}

M.ID = "general"
M.TITLE = "General"

search.index(M.ID, ui.t("settings.general.title"), ui.t("settings.general.terms"), {
	{ key = "language", title = ui.t("settings.general.language.label"), terms = ui.t("settings.general.language.terms") },
	{ key = "send_with", title = ui.t("settings.general.send_with.label"), terms = ui.t("settings.general.send_with.terms") },
	{ key = "compact_mode", title = ui.t("settings.general.compact_mode.label"), terms = ui.t("settings.general.compact_mode.terms") },
	{ key = "escape_stops", title = ui.t("settings.general.escape_stops.label"), terms = ui.t("settings.general.escape_stops.terms") },
	{ key = "close_panel", title = ui.t("settings.general.close_panel.label"), terms = ui.t("settings.general.close_panel.terms") },
	{ key = "hide_emails", title = ui.t("settings.general.hide_emails.label"), section = ui.t("settings.general.privacy"), terms = ui.t("settings.general.hide_emails.terms") },
	{ key = "thread_naming", title = ui.t("settings.general.thread_naming.label"), terms = ui.t("settings.general.thread_naming.terms") },
	{ key = "worktrees", title = ui.t("settings.general.worktrees.label"), section = ui.t("settings.general.git"), terms = ui.t("settings.general.worktrees.terms") },
	{ key = "commit_credit", title = ui.t("settings.general.commit_credit.label"), section = ui.t("settings.general.git"), terms = ui.t("settings.general.commit_credit.terms") },
})

-- The engine's settings for this Mac, as last heard: nil until asked.
type Credit = { enabled: boolean, cloud: boolean? }
type Worktrees = {
	settings: { useCustomDirectory: boolean?, customDirectory: string? },
	defaultDirectory: string?,
	effectiveDirectory: string?,
	environmentOverride: string?,
}
type Titles = { harness: string?, model: string? }
local credit = ui.state("settings_commit_credit", nil :: Credit?)
local worktrees = ui.state("settings_worktrees", nil :: Worktrees?)
local titles = ui.state("settings_titles", nil :: Titles?)
-- Why one couldn't be had (an outcome word), by name.
local failed = ui.state("settings_engine_failed", {} :: { [string]: string })

local function engine(method: string, params: { [string]: any }?): any
	return host.call("settings.engine", { method = method, params = params or {} })
end

local function says(word: string?): Message
	if word == "outdated" then
		return ui.t("settings.engine.outdated")
	elseif word == "offline" then
		return ui.t("settings.engine.offline")
	end
	return ui.t("common.something_went_wrong")
end

local function load(name: string, method: string, into: State<any>)
	engine(method):after(function(v: any)
		into:set(v)
		failed:set_key(name, nil)
	end, function(err: any)
		failed:set_key(name, err.message)
	end)
end

local function load_all()
	load("credit", "GetCommitCredit", credit)
	load("worktrees", "GetWorktreeSettings", worktrees)
	load("titles", "GetTitleSettings", titles)
end

-- A change to the engine's: shown now, back as it was if refused.
local function change(name: string, into: State<any>, next: any, method: string, params: { [string]: any })
	local before = into:peek()
	into:set(next)
	engine(method, params):after(function(v: any)
		if v ~= nil then
			into:set(v)
		end
	end, function(err: any)
		into:set(before)
		toast.show { title = ui.t("settings.engine.couldnt_save"):get(), body = says(err.message):get(), tone = "danger" }
	end)
end

-- A switch on the engine's setting: it follows what the engine says, and
-- a flip goes to it.
local function engine_switch(label: Message, read: () -> boolean?, write: (on: boolean) -> (), disabled: (() -> boolean)?): Node
	return ui.component(function(): Node
		local on = ui.use_state(read() or false)
		ui.effect(function()
			local v = read()
			if v ~= nil and v ~= on:peek() then
				on:set(v)
			end
		end)
		return ui.div {
			opacity = function()
				return if disabled and disabled() then "$zeron.disabled" else 1
			end,
			disabled = function()
				return disabled ~= nil and disabled()
			end,
			ds.Switch {
				value = on,
				label = label,
				on_change = function(v: boolean)
					-- (Not this Mac's to change: it stays as the engine says.)
					if disabled and disabled() then
						on:set(not v)
						return
					end
					write(v)
				end,
			},
		}
	end)({})
end

-- Thread naming: the session's own agent, or one model for every title.
local AUTO = "auto"
local function title_value(): string
	local t = titles:get()
	if t and t.harness and t.model then
		return `{t.harness}/{t.model}`
	end
	return AUTO
end
local function naming(): Node
	-- (The choices are the models offered, followed as they arrive: built
	-- once.)
	local function options(): { any }
		local out: { any } = { { value = AUTO, label = ui.t("settings.general.thread_naming.session_agent"):get() } }
		local catalog = chat.catalog:get()
		if catalog.state ~= "ready" then
			return out
		end
		local names: { [string]: string } = {}
		for _, h in catalog.harnesses do
			names[h.id] = h.label
		end
		for _, m in catalog.models do
			table.insert(out, { value = m.value, label = m.label, description = names[m.harness] })
		end
		return out
	end
	return ui.component(function(): Node
		local chosen = ui.use_state(title_value())
		ui.effect(function()
			local v = title_value()
			if v ~= chosen:peek() then
				chosen:set(v)
			end
		end)
		return ds.Select {
			value = chosen,
			name = ui.t("settings.general.thread_naming.label"),
			w = 200,
			options = options,
			on_change = function(v: string)
				if v == AUTO then
					change("titles", titles, {}, "SetTitleSettings", {})
					return
				end
				local harness, model = string.match(v, "^([^/]+)/(.+)$")
				change("titles", titles, { harness = harness, model = model }, "SetTitleSettings", { harness = harness, model = model })
			end,
		}
	end)()
end

-- The worktree folder: off, worktrees go where Zeron puts them; on, under
-- the folder typed (kept even while it's off).
local folder = ui.state("settings_worktree_folder", "")
local function custom(): boolean
	local w = worktrees:get()
	return w ~= nil and w.settings.useCustomDirectory == true
end
local function set_worktrees(on: boolean, dir: string?)
	local w = worktrees:peek()
	if not w then
		return
	end
	local settings = { useCustomDirectory = on, customDirectory = dir or w.settings.customDirectory }
	local next = table.clone(w) :: any
	next.settings = settings
	change("worktrees", worktrees, next, "SetWorktreeSettings", settings)
end
ui.effect(function()
	local w = worktrees:get()
	local dir = if w then w.settings.customDirectory else nil
	if dir and folder:peek() == "" then
		folder:set(dir)
	end
end)

local function row(key: string, props: any): Node
	return search.anchor(key, ds.Settings.Row(props))
end

function M.view(): Node
	visits.once("general", function()
		load_all()
		if chat.catalog:peek().state ~= "ready" then
			chat.load_catalog()
		end
	end)
	return ds.Settings.Page {
		title = ui.t("settings.general.title"),
		ds.Settings.Section {
			ds.Settings.Group {
				row("language", {
					label = ui.t("settings.general.language.label"),
					description = function()
						return if language.chosen:get() == "system"
							then ui.t("settings.general.language.system"):get()
							else ui.t("settings.general.language.chosen"):get()
					end,
					-- (Its own id: its name is in whichever language it shows.)
					control = ui.div {
						id = "settings-language",
						ds.Select { value = language.chosen, name = ui.t("settings.general.language.label"), w = 180, options = language.options },
					},
				}),
				row("send_with", {
					label = ui.t("settings.general.send_with.label"),
					description = function()
						return if prefs.send_with:get() == "enter"
							then ui.t("settings.general.send_with.enter"):get()
							else ui.t("settings.general.send_with.mod_enter"):get()
					end,
					control = ds.Select { value = prefs.send_with, name = ui.t("settings.general.send_with.label"), w = 128, options = prefs.SEND_WITH },
				}),
				row("compact_mode", {
					label = ui.t("settings.general.compact_mode.label"),
					description = ui.t("settings.general.compact_mode.description"),
					control = ds.Switch { value = prefs.compact_transcript, label = ui.t("settings.general.compact_mode.label") },
				}),
				row("escape_stops", {
					label = ui.t("settings.general.escape_stops.label"),
					description = ui.t("settings.general.escape_stops.description"),
					control = ds.Switch { value = prefs.escape_stops, label = ui.t("settings.general.escape_stops.label") },
				}),
				row("close_panel", {
					label = ui.t("settings.general.close_panel.label"),
					description = ui.t("settings.general.close_panel.description"),
					control = ds.Switch { value = prefs.close_panel_with_last_tab, label = ui.t("settings.general.close_panel.label") },
				}),
			},
		},
		ds.Settings.Section {
			title = ui.t("settings.general.privacy"),
			ds.Settings.Group {
				row("hide_emails", {
					label = ui.t("settings.general.hide_emails.label"),
					description = ui.t("settings.general.hide_emails.description"),
					control = ds.Switch { value = prefs.hide_emails, label = ui.t("settings.general.hide_emails.label") },
				}),
			},
		},
		ds.Settings.Section {
			ds.Settings.Group {
				row("thread_naming", {
					label = ui.t("settings.general.thread_naming.label"),
					description = function()
						if failed:get().titles then
							return says(failed:get().titles):get()
						end
						return if title_value() == AUTO
							then ui.t("settings.general.thread_naming.auto"):get()
							else ui.t("settings.general.thread_naming.model"):get()
					end,
					control = naming(),
				}),
			},
		},
		ds.Settings.Section {
			title = ui.t("settings.general.git"),
			description = ui.t("settings.general.git_description"),
			ds.Settings.Group {
				-- The switch, and the folder under it while it's on (one
				-- row of the card: the rule between them comes and goes
				-- with the folder).
				ui.column { w_full = true, row("worktrees", {
					label = ui.t("settings.general.worktrees.label"),
					-- (A folder's path runs long: it fades, the switch stays.)
					truncate = true,
					description = function()
						if failed:get().worktrees then
							return says(failed:get().worktrees):get()
						end
						local w = worktrees:get()
						if not w then
							return ui.t("common.loading"):get()
						elseif w.environmentOverride then
							return ui.t("settings.general.worktrees.env", { path = w.environmentOverride }):get()
						end
						local dir = w.effectiveDirectory or w.defaultDirectory
						return if dir
							then ui.t("settings.general.worktrees.in", { path = dir }):get()
							else ui.t("settings.general.worktrees.in_zeron"):get()
					end,
					control = engine_switch(ui.t("settings.general.worktrees.label"), function()
						local w = worktrees:get()
						return if w then w.settings.useCustomDirectory == true else nil
					end, function(on: boolean)
						set_worktrees(on, if folder:peek() ~= "" then folder:peek() else nil)
					end, function()
						local w = worktrees:get()
						return w == nil or w.environmentOverride ~= nil
					end),
				}), ui.show(custom, ui.column {
					w_full = true,
					ds.Settings.Rule(),
					ds.Settings.Row {
					label = ui.t("settings.general.worktrees.folder"),
					description = ui.t("settings.general.worktrees.folder_hint"),
					stacked = true,
					control = ds.TextField {
						value = folder,
						name = ui.t("settings.general.worktrees.folder_name"),
						placeholder = "~/worktrees",
						w_full = true,
						on_commit = function(text: string)
							local dir = string.match(text, "^%s*(.-)%s*$") or ""
							if dir ~= "" then
								set_worktrees(true, dir)
							end
						end,
					},
				},
				}) },
				row("commit_credit", {
					label = ui.t("settings.general.commit_credit.label"),
					description = function()
						if failed:get().credit then
							return says(failed:get().credit):get()
						end
						local c = credit:get()
						if not c then
							return ui.t("common.loading"):get()
						end
						return if c.cloud
							then ui.t("settings.general.commit_credit.cloud"):get()
							else ui.t("settings.general.commit_credit.on"):get()
					end,
					control = engine_switch(ui.t("settings.general.commit_credit.label"), function()
						local c = credit:get()
						return if c then c.enabled else nil
					end, function(on: boolean)
						local c = credit:peek()
						change("credit", credit, { enabled = on, cloud = if c then c.cloud else false }, "SetCommitCredit", { enabled = on })
					end, function()
						local c = credit:get()
						return c == nil or c.cloud == true
					end),
				}),
			},
		},
	}
end

return M

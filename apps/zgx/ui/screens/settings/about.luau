-- Settings → About & updates: which Zeron this is, and its updates, as the
-- shipping app has them (its About panel and "Check for Updates…", crates/ui
-- app_menus.rs, app_update.rs; the engine's updater, crates/update).
--
-- As the Zeron app (the beta: `self_updating`), it updates itself as
-- Zeron.app does (src/about.rs): a new version downloads in the background,
-- "Restart to update" installs it, and one nobody restarted for installs on
-- quit. "Get beta updates" chooses the release channel; leaving beta goes
-- back to Stable even when that's an older version.
--
-- A development build isn't packaged: what updates is the engine's updater,
-- the engine on this Mac, which knows its version and the newest release
-- and, on an install it manages, updates itself and starts again on it.
--
-- The host's `about` (shared): { app_version, app_build, engine_version?,
-- engine_build?, latest?, available, checked_at?, error?, self_updating,
-- channels, channel?, channel_note?, beta_minimum?, held?, switch, flow,
-- flow_version?, flow_error?, blocker? }.

local ds = require("@ds")
local host = require("@app/screens/settings/host")
local search = require("@app/screens/settings/search")
local locale = require("@ds/locale")

local M = {}

M.ID = "about"
M.TITLE = "About & updates"

M.RELEASES = "https://github.com/zeronsh/comet/releases"
M.WEBSITE = "https://zeron.sh"

search.index(M.ID, ui.t("settings.about.title"), ui.t("settings.about.terms"), {
	{ key = "version", title = ui.t("settings.about.version.label"), terms = ui.t("settings.about.version.terms") },
	{
		key = "updates",
		title = ui.t("settings.about.updates.label"),
		section = ui.t("settings.about.updates_section"),
		terms = ui.t("settings.about.updates.terms"),
	},
	{
		key = "release_notes",
		title = ui.t("settings.about.release_notes.label"),
		section = ui.t("settings.about.links_section"),
		terms = ui.t("settings.about.release_notes.terms"),
	},
	{
		key = "beta",
		title = ui.t("settings.about.beta.label"),
		section = ui.t("settings.about.updates_section"),
		terms = ui.t("settings.about.beta.terms"),
	},
	{ key = "website", title = ui.t("settings.about.website.label"), section = ui.t("settings.about.links_section"), terms = ui.t("settings.about.website.terms") },
	{ key = "copy_version", title = ui.t("settings.about.copy_version.label"), terms = ui.t("settings.about.copy_version.terms") },
})

export type About = {
	app_version: string,
	app_build: string?,
	engine_version: string?,
	engine_build: string?,
	latest: string?,
	available: boolean,
	checked_at: number?,
	error: string?,
	self_updating: boolean?,
	channels: boolean?,
	channel: string?,
	channel_note: string?,
	beta_minimum: string?,
	held: string?,
	switch: boolean?,
	flow: string?,
	flow_version: string?,
	flow_error: string?,
	blocker: string?,
}

M.now = ui.shared("about", { app_version = "", available = false } :: About)

-- What this window is doing about an update ("" nothing): "checking",
-- "updating", "updated" (the engine starting again on `M.version`), or how
-- the last try ended: "current", "unmanaged", "offline", "failed".
M.doing = ui.state("about_doing", "")
-- The version an update went to.
local updated_to = ui.state("about_updated_to", "")

-- An engine build ("<commit>.<seconds>") or this app's ("<seconds>"), as
-- people read it: the commit's first seven, the day it was built (in the
-- locale shown: read where it's drawn).
function M.build(b: string?): string?
	if not b or b == "" then
		return nil
	end
	local commit, secs = string.match(b, "^(%x+)%.(%d+)$")
	if not secs then
		secs = string.match(b, "^(%d+)$")
	end
	local n = tonumber(secs)
	local day: string? = nil
	if n and n > 0 then
		day = locale.date(n)
	end
	if commit and day then
		return ui.t("settings.about.build", { commit = string.sub(commit, 1, 7), day = day }):get()
	end
	return day or (if commit then string.sub(commit, 1, 7) else b)
end

-- When the newest release was last asked, said shortly.
local function ago(ms: number?): string?
	if not ms then
		return nil
	end
	local s = math.max(0, os.time() - math.floor(ms / 1000))
	if s < 60 then
		return ui.t("settings.about.ago.just_now"):get()
	elseif s < 3600 then
		return ui.t("settings.about.ago.minutes", { n = math.floor(s / 60) }):get()
	elseif s < 86400 then
		return ui.t("settings.about.ago.hours", { n = math.floor(s / 3600) }):get()
	end
	return ui.t("settings.about.ago.days", { n = math.floor(s / 86400) }):get()
end

function M.check()
	if M.doing:peek() == "checking" or M.doing:peek() == "updating" then
		return
	end
	M.doing:set("checking")
	host.call("about.check"):after(function()
		M.doing:set("")
	end, function(err: any)
		M.doing:set(if err.message == "offline" then "offline" else "failed")
	end)
end

function M.update()
	if M.doing:peek() == "updating" then
		return
	end
	M.doing:set("updating")
	host.call("about.apply"):after(function(answer: any)
		updated_to:set(answer and answer.version or "")
		M.doing:set("updated")
	end, function(err: any)
		local word = err.message
		-- (Held: the page says why from `about.held`.)
		M.doing:set(if word == "held" then "" elseif word == "unmanaged" or word == "current" or word == "offline" then word else "failed")
	end)
end

-- "Get beta updates": the host answers with the channel it chose (or why
-- not, `channel_note`), and the switch follows that.
local beta = ui.state("about_beta", false)
ui.effect(function()
	beta:set(M.now:get().channel == "beta")
end)
function M.set_beta(on: boolean)
	host.invoke("about.channel", on)
end

-- What the update row says now: its title, its line, and its action (in
-- the locale shown: read where it's drawn).
export type Update = { title: string, line: string, action: string?, accent: boolean? }

function M.describe(a: About, doing: string): Update
	local current = a.engine_version
	local checked = ago(a.checked_at)
	local function s(key: string, args: { [string]: any }?): string
		return ui.t(`settings.about.state.{key}`, args):get()
	end
	local retry = ui.t("common.retry"):get()
	if a.self_updating then
		-- The Zeron app's own updates (Zeron.app's lifecycle).
		local mine = a.app_version
		if doing == "checking" then
			return { title = s("checking"), line = s("checking_line") }
		elseif a.flow == "downloading" then
			return { title = s("downloading", { version = a.flow_version or "" }), line = s("downloading_line") }
		elseif a.flow == "ready" then
			return { title = s("ready", { version = a.flow_version or "" }), line = s("ready_line"), action = s("restart"), accent = true }
		elseif a.flow == "failed" then
			return {
				title = s("download_failed", { version = a.flow_version or "" }),
				line = a.flow_error or s("failed_line"),
				action = retry,
				accent = true,
			}
		elseif a.held and a.latest then
			return { title = s("held", { version = a.latest }), line = a.held, action = s("check_again") }
		elseif doing == "offline" then
			return { title = s("couldnt_check"), line = s("offline_line"), action = retry }
		elseif doing == "failed" then
			return { title = s("couldnt_update"), line = s("failed_line"), action = retry }
		elseif a.available and a.latest then
			local title = if a.switch then s("available_stable", { version = a.latest }) else s("available", { version = a.latest })
			return { title = title, line = s("you_have", { version = mine }), action = s("update"), accent = true }
		elseif a.error and not checked then
			return { title = s("couldnt_check"), line = s("unreachable_line"), action = retry }
		end
		return {
			title = s("up_to_date"),
			line = if checked then s("newest_checked", { version = mine, ago = checked }) else s("newest", { version = mine }),
			action = s("check"),
		}
	end
	if doing == "checking" then
		return { title = s("checking"), line = s("checking_line") }
	elseif doing == "updating" then
		return { title = s("updating"), line = s("updating_line", { version = a.latest or "" }) }
	elseif doing == "updated" then
		local v = updated_to:peek()
		if current and v ~= "" and current == v then
			return { title = s("updated", { version = v }), line = s("updated_running") }
		end
		return { title = s("updated", { version = v }), line = s("updated_starting") }
	elseif doing == "unmanaged" then
		return {
			title = s("unmanaged"),
			line = s("unmanaged_line"),
			action = s("check_again"),
		}
	elseif doing == "offline" then
		return { title = s("couldnt_check"), line = s("offline_line"), action = retry }
	elseif doing == "failed" then
		return { title = s("couldnt_update"), line = s("failed_line"), action = retry }
	end
	if not current then
		return { title = s("not_connected"), line = s("not_connected_line"), action = s("check") }
	elseif a.available and a.latest then
		return { title = s("available", { version = a.latest }), line = s("you_have", { version = current }), action = s("update_engine"), accent = true }
	elseif a.error and not checked then
		return { title = s("couldnt_check"), line = s("unreachable_line"), action = retry }
	end
	return {
		title = s("up_to_date"),
		line = if checked then s("newest_checked", { version = current, ago = checked }) else s("newest", { version = current }),
		action = s("check"),
	}
end

-- The version facts as text, for a bug report (in English, as support
-- reads them).
function M.summary(a: About): string
	local lines = {
		`Zeron engine: {a.engine_version or "not connected"}`,
	}
	if a.engine_build then
		table.insert(lines, `Engine build: {a.engine_build}`)
	end
	table.insert(lines, `App: zgx {a.app_version}{if a.app_build and a.app_build ~= "" then ` (built {a.app_build})` else ""}`)
	if a.latest then
		table.insert(lines, `Newest release: {a.latest}`)
	end
	return table.concat(lines, "\n")
end

local function row(key: string, props: any): Node
	return search.anchor(key, ds.Settings.Row(props))
end

local function fact(text: () -> string): Node
	return ui.text {
		text,
		text_size = "$font.size.md",
		line_height = "$font.line.md",
		color = "$text.secondary",
		tabular = true,
	}
end

local function now_update(): Update
	return M.describe(M.now:get(), M.doing:get())
end

function M.view(): Node
	return ds.Settings.Page {
		title = ui.t("settings.about.title"),
		description = ui.t("settings.about.description"),
		ds.Settings.Section {
			ds.Settings.Group {
				row("version", {
					label = "Zeron",
					description = function(): string
						return M.build(M.now:get().engine_build) or ui.t("settings.about.version.description"):get()
					end,
					control = fact(function(): string
						return M.now:get().engine_version or "—"
					end),
				}),
				row("app_version", {
					label = ui.t("settings.about.app.label"),
					description = function(): string
						local built = M.build(M.now:get().app_build)
						if M.now:get().self_updating and built then
							return ui.t("settings.about.app.packaged", { built = built }):get()
						end
						return if built
							then ui.t("settings.about.app.built", { built = built }):get()
							else ui.t("settings.about.app.not_packaged"):get()
					end,
					control = fact(function(): string
						local v = M.now:get().app_version
						return if v ~= "" then v else "—"
					end),
				}),
				row("copy_version", {
					label = ui.t("settings.about.version_info.label"),
					description = ui.t("settings.about.version_info.description"),
					control = ds.Button {
						ui.t("common.copy"),
						tone = "secondary",
						size = "sm",
						on_click = function()
							ui.clipboard.write(M.summary(M.now:peek()))
							ds.toast.show { title = ui.t("settings.about.version_info.copied"):get(), tone = "success" }
						end,
					},
				}),
			},
		},
		ds.Settings.Section {
			title = ui.t("settings.about.updates_section"),
			-- (Whether the app updates itself is fixed for the process.)
			description = if M.now:peek().self_updating
				then ui.t("settings.about.updates_self_description")
				else ui.t("settings.about.updates_description"),
			ds.Settings.Group {
				ui.column {
					w_full = true,
					row("updates", {
						label = function(): string
							return now_update().title
						end,
						description = function(): string
							return now_update().line
						end,
						-- (Updating is the one accented action; the rest ask again.)
						control = ui.row {
							items = "center",
							ui.show(function(): boolean
								return now_update().accent == true
							end, ds.Button {
								function(): string
									return now_update().action or ""
								end,
								tone = "accent",
								size = "sm",
								on_click = M.update,
							}),
							ui.show(function(): boolean
								local u = now_update()
								return u.action ~= nil and not u.accent
							end, ds.Button {
								function(): string
									return now_update().action or ""
								end,
								tone = "secondary",
								size = "sm",
								on_click = M.check,
							}),
						},
					}),
					-- The release channel (a macOS app install).
					-- (A row that comes and goes brings its own hairline: none under the
					-- one above while it's away.)
					ui.show(function(): boolean
						return M.now:get().channels == true
					end, ui.column {
						w_full = true,
						ds.Settings.Rule(),
						row("beta", {
						label = ui.t("settings.about.beta.label"),
						description = function(): string
							local a = M.now:get()
							if a.channel_note then
								return a.channel_note
							end
							return if a.channel == "beta"
								then ui.t("settings.about.beta.on"):get()
								else ui.t("settings.about.beta.off", { version = a.beta_minimum or "13.0" }):get()
						end,
						control = ds.Switch { value = beta, label = ui.t("settings.about.beta.label"), on_change = M.set_beta },
						}),
					}),
				},
			},
		},
		ds.Settings.Section {
			title = ui.t("settings.about.links_section"),
			ds.Settings.Group {
				row("release_notes", {
					label = ui.t("settings.about.release_notes.label"),
					description = ui.t("settings.about.release_notes.description"),
					on_click = function()
						local v = M.now:peek().latest or M.now:peek().engine_version
						host.invoke("open_url", if v then `{M.RELEASES}/tag/v{v}` else M.RELEASES)
					end,
					href = function(): string
						local v = M.now:peek().latest or M.now:peek().engine_version
						return if v then `{M.RELEASES}/tag/v{v}` else M.RELEASES
					end,
				}),
				row("website", {
					label = ui.t("settings.about.website.label"),
					description = "zeron.sh",
					on_click = function()
						host.invoke("open_url", M.WEBSITE)
					end,
					href = M.WEBSITE,
				}),
			},
		},
	}
end

return M

-- A device's repositories, and the pieces every flow that picks or makes
-- one shares: New project's "From repositories", Edit project's
-- Repositories, and Settings → Repositories.
--
--   repositories.library(device)   the device's repositories as its host
--                                  finds them (a look through its home, and
--                                  those added, cloned or made through
--                                  Zeron), newest worked-in first: states
--                                  `list`, `loading`, `failed`, `parent`
--                                  (where a new one goes), `home`,
--                                  `publish` (its GitHub CLI is signed in),
--                                  and `load(force?)` (asked once, then
--                                  again only when forced or stale)
--   repositories.Picker { … }      a search over them and a check on each
--                                  chosen (a path typed adds the repository
--                                  there)
--   repositories.NewForm(device)   a new repository's name, where it goes,
--                                  a first commit, GitHub; `submit_new`
--   repositories.CloneForm(device) an address and where it goes;
--                                  `submit_clone`
--   repositories.Dialog()          New repository / Clone repository on
--                                  their own (Settings, the palette); placed
--                                  once in the window, opened by
--                                  `open_new(device, done?)` and
--                                  `open_clone(device, done?)`
--   repositories.tilde(path, home) a path as it reads (`~/code/api`)
--
-- A repository is `{ path, name, branch?, empty?, github?, touched?, kept? }`.

local ds = require("@ds")
local Icon = require("@ds/icon")
local FadeText = require("@ds/fade_text")
local Skeleton = require("@ds/skeleton")
local ListRow = require("@ds/list_row").ListRow
local control = require("@ds/control")
local locale = require("@ds/locale")
local toast = require("@ds/toast")
local sessions = require("@account/sessions")
local projects = require("@account/projects")

local M = {}

export type Repository = {
	path: string,
	name: string,
	branch: string?,
	empty: boolean?,
	github: string?,
	touched: number?,
	kept: boolean?,
}

-- How long a device's list is trusted before it's asked for again.
local STALE = 60

export type Library = {
	list: State<{ Repository }>,
	loading: State<boolean>,
	failed: State<string?>,
	parent: State<string>,
	home: State<string>,
	publish: State<boolean>,
	load: (force: boolean?) -> (),
	-- One just made, cloned or added, listed first at once.
	add: (repository: Repository) -> (),
}

-- The engine's refusal in the flows' words.
function M.says(err: any): string
	local text = if type(err) == "string"
		then err
		elseif type(err) == "table" and err.message then tostring(err.message)
		else locale.s("shell.repositories.error.unknown")
	if string.find(text, "not connected", 1, true) or text == "offline" then
		return locale.s("shell.repositories.error.offline")
	end
	-- (The engine's own words, without its "git clone failed (…):" frame.)
	return (string.gsub(text, "^git %a+ failed %b():%s*", ""))
end

function M.tilde(path: string, home: string?): string
	if home and home ~= "" and string.sub(path, 1, #home) == home then
		local rest = string.sub(path, #home + 1)
		if rest == "" then
			return "~"
		end
		local first = string.sub(rest, 1, 1)
		if first == "/" or first == "\\" then
			return "~" .. rest
		end
	end
	return path
end

local libraries: { [string]: Library } = {}
function M.library(device: string): Library
	local found = libraries[device]
	if found then
		return found
	end
	local key = `shell_repositories_{device}`
	local list = ui.state(`{key}_list`, {} :: { Repository })
	local loading = ui.state(`{key}_loading`, false)
	local failed = ui.state(`{key}_failed`, nil :: string?)
	local parent = ui.state(`{key}_parent`, "")
	local home = ui.state(`{key}_home`, "")
	local publish = ui.state(`{key}_publish`, false)
	local asked_at = -math.huge
	local asking = 0
	local library: Library
	library = {
		list = list,
		loading = loading,
		failed = failed,
		parent = parent,
		home = home,
		publish = publish,
		load = function(force: boolean?)
			if not force and (loading:peek() or os.clock() - asked_at < STALE) then
				return
			end
			asked_at = os.clock()
			asking += 1
			local mine = asking
			loading:set(true)
			projects.local_repositories(device, force):after(function(found: any)
				if mine ~= asking then
					return
				end
				ui.batch(function()
					loading:set(false)
					failed:set(nil)
					if type(found) == "table" then
						list:set(found.list or {})
						parent:set(found.parent or "")
						home:set(found.home or "")
						publish:set(found.publish == true)
					end
				end)
			end, function(err: any)
				if mine ~= asking then
					return
				end
				asked_at = -math.huge
				ui.batch(function()
					loading:set(false)
					failed:set(M.says(err))
				end)
			end)
		end,
		add = function(repository: Repository)
			local next: { Repository } = { repository }
			for _, r in list:peek() do
				if r.path ~= repository.path then
					table.insert(next, r)
				end
			end
			list:set(next)
		end,
	}
	libraries[device] = library
	return library
end

-- Whether `text` is a path (absolute, home's, a drive's).
local function typed_path(text: string): boolean
	return string.sub(text, 1, 1) == "/"
		or text == "~"
		or string.match(text, "^~[/\\]") ~= nil
		or string.match(text, "^%a:[\\/]") ~= nil
end

-- The repositories matching `query`: by name first (its start, then
-- anywhere), then by path or GitHub repository. All, in order, for none.
function M.matching(list: { Repository }, query: string): { Repository }
	local q = string.lower(string.match(query, "^%s*(.-)%s*$") or "")
	if q == "" then
		return list
	end
	local starts, inside, elsewhere = {}, {}, {}
	for _, r in list do
		local name = string.lower(r.name)
		if string.sub(name, 1, #q) == q then
			table.insert(starts, r)
		elseif string.find(name, q, 1, true) then
			table.insert(inside, r)
		elseif string.find(string.lower(r.path), q, 1, true) or (r.github and string.find(string.lower(r.github), q, 1, true)) then
			table.insert(elsewhere, r)
		end
	end
	for _, r in inside do
		table.insert(starts, r)
	end
	for _, r in elsewhere do
		table.insert(starts, r)
	end
	return starts
end

-- ── one repository's row ────────────────────────────────────────────────

-- Its branch and its GitHub repository, quiet, at its end.
local function facts(r: () -> Repository): Node
	return ui.row {
		shrink = 0,
		max_w = 200,
		gap = "$space.3",
		items = "center",
		color = "$text.tertiary",
		ui.show(function(): boolean
			return r().github ~= nil
		end, FadeText {
			function(): string
				return r().github or ""
			end,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
			max_w = 120,
		}),
		ui.row {
			gap = "$space.1",
			items = "center",
			ui.show(function(): boolean
				return r().branch ~= nil and not r().empty
			end, Icon { name = "git-branch", size = 12 }),
			ui.text {
				function(): string
					local it = r()
					if it.empty then
						return locale.s("shell.repositories.no_commits")
					end
					return it.branch or locale.s("shell.repositories.detached")
				end,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
				truncate = true,
				max_w = 110,
			},
		},
	}
end
M.facts = facts

-- Its name over where it is.
local function names(r: () -> Repository, home: () -> string): Node
	return ui.column {
		grow = 1,
		min_w = 0,
		gap = 1,
		FadeText {
			function(): string
				return r().name
			end,
			text_size = "$font.size.md",
			weight = "$font.weight.medium",
			color = "$text.primary",
		},
		FadeText {
			function(): string
				return M.tilde(r().path, home())
			end,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
			literal = true,
		},
	}
end
M.names = names

-- ── the picker ──────────────────────────────────────────────────────────

-- Rows shown before the list scrolls, and a row's height.
local ROWS = 6
local ROW_H = 46

export type PickerProps = {
	id: string,
	device: () -> string,
	-- Chosen, by path (kept by the caller).
	chosen: State<{ [string]: boolean }>,
	-- Those not to offer (already in the project), by path.
	hidden: (() -> { [string]: boolean })?,
	-- "New repository" and "Clone": the caller shows their forms.
	on_new: () -> (),
	on_clone: () -> (),
	query: State<string>,
	-- Rows before it scrolls.
	rows: number?,
}

function M.Picker(props: PickerProps): Node
	local function lib(): Library
		return M.library(props.device())
	end
	local adding = ui.state(`{props.id}_adding`, false)
	local add_failed = ui.state(`{props.id}_add_failed`, nil :: string?)
	ui.effect(function()
		props.query:get()
		add_failed:set(nil)
	end)
	local function offered(): { Repository }
		local hidden = if props.hidden then props.hidden() else {}
		local out = {}
		for _, r in lib().list:get() do
			if not hidden[r.path] then
				table.insert(out, r)
			end
		end
		return out
	end
	-- (As `offered`, read now: for a key's handler.)
	local function offered_now(): { Repository }
		local hidden = if props.hidden then props.hidden() else {}
		local out = {}
		for _, r in lib().list:peek() do
			if not hidden[r.path] then
				table.insert(out, r)
			end
		end
		return out
	end
	local function shown(): { Repository }
		local q = props.query:get()
		if typed_path(q) then
			return {}
		end
		return M.matching(offered(), q)
	end
	local function toggle(path: string)
		local now = props.chosen:peek()
		props.chosen:set_key({ path }, if now[path] then nil else true)
	end
	local function chosen_count(): number
		local n = 0
		for _ in props.chosen:get() do
			n += 1
		end
		return n
	end

	-- A path typed: that repository, added to the device's list and chosen.
	local function add_typed()
		local q = string.match(props.query:peek(), "^%s*(.-)%s*$") or ""
		if q == "" or adding:peek() then
			return
		end
		adding:set(true)
		add_failed:set(nil)
		local device = props.device()
		projects.add_repository(device, q):after(function(repo: any)
			adding:set(false)
			if type(repo) ~= "table" then
				return
			end
			lib().add({ path = repo.path, name = repo.name, kept = true })
			props.chosen:set_key({ repo.path }, true)
			props.query:set("")
			lib().load(true)
		end, function(err: any)
			adding:set(false)
			add_failed:set(M.says(err))
		end)
	end

	local function row(r: Repository, _i: number, live: State<Repository>): Node
		local path = r.path
		local function now(): Repository
			return live:get()
		end
		local function on(): boolean
			return props.chosen:get()[path] == true
		end
		return ListRow {
			class = "repository-pick",
			role = "checkbox",
			label = r.name,
			h = ROW_H,
			on_click = function()
				toggle(path)
			end,
			ui.row {
				w_full = true,
				h_full = true,
				px = "$space.3",
				gap = "$space.3",
				items = "center",
				ds.Checkbox {
					value = on,
					on_change = function()
						toggle(path)
					end,
					bare = true,
					name = r.name,
				},
				names(now, function(): string
					return lib().home:get()
				end),
				facts(now),
			},
		}
	end

	local rows = ui.each(ui.computed(shown), row, {
		key = function(r: Repository): string
			return r.path
		end,
		w_full = true,
	})
	local most = props.rows or ROWS

	local function state(): string
		local l = lib()
		if typed_path(props.query:get()) then
			return "path"
		elseif #l.list:get() == 0 and l.loading:get() then
			return "loading"
		elseif #l.list:get() == 0 and l.failed:get() then
			return "failed"
		elseif #offered() == 0 then
			return "none"
		elseif #shown() == 0 then
			return "unmatched"
		end
		return "list"
	end

	local well = function(child: Node): Node
		return ui.column {
			w_full = true,
			h = most * ROW_H,
			rounded = "$radius.lg",
			bg = "$surface.sunken",
			border = 1,
			border_color = "$border.subtle",
			overflow = "hidden",
			child,
		}
	end

	local body = ui.switch(state, {
		loading = well(ui.column {
			w_full = true,
			label = locale.t("shell.repositories.looking"),
			live = "polite",
			Skeleton.Row { lead = 16, title = 90, line = 170, trail = 48 },
			Skeleton.Row { lead = 16, title = 130, line = 210, trail = 48 },
			Skeleton.Row { lead = 16, title = 70, line = 150, trail = 48 },
			Skeleton.Row { lead = 16, title = 110, line = 190, trail = 48 },
		}),
		failed = well(ds.EmptyState {
			icon = "wifi-off",
			size = "sm",
			fill = true,
			title = function(): string
				return locale.s("shell.repositories.failed.title")
			end,
			body = function(): string
				return lib().failed:get() or ""
			end,
			action = {
				label = locale.t("shell.repositories.failed.retry"),
				on_click = function()
					lib().load(true)
				end,
			},
		}),
		none = well(ds.EmptyState {
			icon = "folder",
			size = "sm",
			fill = true,
			title = function(): string
				return if #lib().list:get() == 0
					then locale.s("shell.repositories.none.title")
					else locale.s("shell.repositories.none.all_in")
			end,
			body = locale.t("shell.repositories.none.body"),
			action = { label = locale.t("shell.repositories.new"), on_click = props.on_new },
			secondary = { label = locale.t("shell.repositories.clone"), on_click = props.on_clone },
		}),
		unmatched = well(ds.EmptyState {
			icon = "search",
			size = "sm",
			fill = true,
			title = function(): string
				return locale.s("shell.repositories.unmatched", { query = props.query:get() })
			end,
			body = locale.t("shell.repositories.unmatched_body"),
		}),
		path = well(ui.column {
			w_full = true,
			h_full = true,
			items = "center",
			justify = "center",
			gap = "$space.3",
			px = "$space.6",
			ui.text {
				function(): string
					return locale.s("shell.repositories.path.title", { path = string.match(props.query:get(), "^%s*(.-)%s*$") or "" })
				end,
				text_size = "$font.size.md",
				color = "$text.secondary",
				wrap = true,
				align = "center",
				literal = true,
			},
			ui.show(function(): boolean
				return add_failed:get() ~= nil
			end, ui.text {
				function(): string
					return add_failed:get() or ""
				end,
				role = "alert",
				text_size = "$font.size.sm",
				color = "$danger",
				wrap = true,
				align = "center",
			}),
			ds.Button {
				locale.t("shell.repositories.path.add") :: any,
				id = `{props.id}-add-path`,
				tone = "secondary",
				size = "sm",
				busy = adding,
				on_click = add_typed,
			},
		}),
		list = ui.column {
			w_full = true,
			rounded = "$radius.lg",
			bg = "$surface.sunken",
			border = 1,
			border_color = "$border.subtle",
			overflow = "hidden",
			ui.scroll {
				id = `{props.id}-list`,
				role = "group",
				label = locale.t("shell.repositories.label"),
				w_full = true,
				overscroll = "contain",
				h = most * ROW_H,
				rows,
			},
		},
	})

	return ui.column {
		id = props.id,
		w_full = true,
		gap = "$space.2",
		ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			ds.SearchField {
				value = props.query,
				name = locale.t("shell.repositories.search"),
				placeholder = locale.t("shell.repositories.search_placeholder"),
				size = "sm",
				grow = 1,
				-- Enter here is the search's, never the dialog's: a path typed
				-- is used, and a search with one match chooses it.
				on_key = function(e: any): boolean?
					if e.key ~= "Enter" or e.cmd or e.ctrl or e.alt or e.shift then
						return nil
					end
					local text = props.query:peek()
					if typed_path(text) then
						add_typed()
						return true
					end
					if string.match(text, "%S") == nil then
						return nil
					end
					local found = M.matching(offered_now(), text)
					if #found == 1 then
						toggle(found[1].path)
					end
					return true
				end,
			},
			ds.Button {
				ui.row { gap = "$space.1", items = "center", Icon { name = "plus", size = 12 }, ui.text { locale.t("shell.repositories.new_short") } } :: any,
				id = `{props.id}-new`,
				tone = "secondary",
				size = "sm",
				label = locale.t("shell.repositories.new"),
				on_click = props.on_new,
			},
			ds.Button {
				ui.row { gap = "$space.1", items = "center", Icon { name = "download", size = 12 }, ui.text { locale.t("shell.repositories.clone_short") } } :: any,
				id = `{props.id}-clone`,
				tone = "secondary",
				size = "sm",
				label = locale.t("shell.repositories.clone"),
				on_click = props.on_clone,
			},
		},
		body,
		ui.row {
			w_full = true,
			h = 16,
			items = "center",
			gap = "$space.2",
			ui.text {
				function(): string
					local n = chosen_count()
					if n == 0 then
						return locale.s("shell.repositories.found", { n = #offered() })
					end
					return locale.s("shell.repositories.chosen", { n = n })
				end,
				id = `{props.id}-count`,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
				tabular = true,
				grow = 1,
			},
			ui.show(function(): boolean
				return lib().loading:get() and #lib().list:get() > 0
			end, ui.text { locale.t("shell.repositories.looking"), text_size = "$font.size.xs", color = "$text.tertiary" }),
			ui.show(function(): boolean
				return chosen_count() > 0
			end, ui.button {
				id = `{props.id}-clear`,
				label = locale.t("shell.repositories.clear"),
				h = 16,
				px = "$space.1",
				rounded = "$radius.sm",
				text_size = "$font.size.xs",
				color = "$accent",
				bg = "#00000000",
				hover = { bg = "$hover" },
				focus = control.ring,
				on_click = function()
					props.chosen:set({})
				end,
				ui.text { locale.t("shell.repositories.clear") },
			}),
		},
	}
end

-- ── New repository ──────────────────────────────────────────────────────

local new_name = ui.state("shell_repositories_new_name", "")
local new_parent = ui.state("shell_repositories_new_parent", "")
local new_readme = ui.state("shell_repositories_new_readme", true)
local new_publish = ui.state("shell_repositories_new_publish", false)
local new_private = ui.state("shell_repositories_new_private", true)
local new_failed = ui.state("shell_repositories_new_failed", nil :: string?)
local new_unnamed = ui.state("shell_repositories_new_unnamed", false)
M.busy = ui.state("shell_repositories_busy", false)

-- The name as the engine will make it (letters, digits, `.`, `_`, `-`).
local function clean_name(text: string): string
	local t = string.match(text, "^%s*(.-)%s*$") or ""
	t = string.gsub(t, "[^%w%._%-]", "-")
	t = string.gsub(t, "^[%-%.]+", "")
	t = string.gsub(t, "[%-%.]+$", "")
	return t
end
M.clean_name = clean_name

function M.reset_new(device: string, name: string?)
	local lib = M.library(device)
	lib.load()
	ui.batch(function()
		new_name:set(name or "")
		new_parent:set("")
		new_readme:set(true)
		new_publish:set(false)
		new_private:set(true)
		new_failed:set(nil)
		new_unnamed:set(false)
		M.busy:set(false)
	end)
end

function M.NewForm(device: () -> string): Node
	local function lib(): Library
		return M.library(device())
	end
	local function where(): string
		local typed = string.match(new_parent:get(), "^%s*(.-)%s*$") or ""
		return if typed ~= "" then typed else M.tilde(lib().parent:get(), lib().home:get())
	end
	return ui.column {
		id = "new-repository",
		w_full = true,
		gap = "$space.4",
		ds.TextField {
			value = new_name,
			label = locale.t("shell.repositories.new_form.name"),
			placeholder = locale.t("shell.repositories.new_form.name_placeholder"),
			autofocus = true,
			w_full = true,
			hint = function(): string
				local clean = clean_name(new_name:get())
				if clean == "" then
					return locale.s("shell.repositories.new_form.where_hint", { path = where() })
				end
				local sep = if string.find(where(), "\\", 1, true) then "\\" else "/"
				local base = where()
				if string.sub(base, -1) ~= sep then
					base ..= sep
				end
				return locale.s("shell.repositories.new_form.at", { path = base .. clean })
			end,
			error = function(): string?
				local why = new_failed:get()
				if new_unnamed:get() then
					why = locale.s("shell.repositories.new_form.unnamed")
				end
				return why
			end,
		},
		ds.TextField {
			value = new_parent,
			label = locale.t("shell.repositories.new_form.location"),
			placeholder = function(): string
				return M.tilde(lib().parent:get(), lib().home:get())
			end,
			icon = "folder",
			w_full = true,
			size = "sm",
		},
		ui.column {
			w_full = true,
			gap = "$space.2",
			ds.Checkbox {
				value = new_readme,
				label = locale.t("shell.repositories.new_form.readme"),
				description = locale.t("shell.repositories.new_form.readme_hint"),
			},
			ui.show(function(): boolean
				return lib().publish:get()
			end, ui.column {
				w_full = true,
				gap = "$space.2",
				ds.Checkbox {
					value = new_publish,
					label = locale.t("shell.repositories.new_form.publish"),
					description = locale.t("shell.repositories.new_form.publish_hint"),
				},
				ui.show(function(): boolean
					return new_publish:get()
				end, ui.div {
					pl = ui.raw(26, "under a checkbox's label: its box and the gap after it"),
					ds.Segmented {
						value = new_private,
						label = locale.t("shell.repositories.new_form.visibility"),
						options = {
							{ value = true, label = locale.t("shell.repositories.new_form.private") :: any },
							{ value = false, label = locale.t("shell.repositories.new_form.public") :: any },
						},
					},
				}),
			}),
		},
	}
end

-- Makes the repository in the form; `done` with it once it's there.
function M.submit_new(device: string, done: (repository: Repository) -> ())
	if M.busy:peek() then
		return
	end
	local name = clean_name(new_name:peek())
	if name == "" then
		new_unnamed:set(true)
		return
	end
	new_unnamed:set(false)
	new_failed:set(nil)
	M.busy:set(true)
	local parent = string.match(new_parent:peek(), "^%s*(.-)%s*$") or ""
	local lib = M.library(device)
	local publish = new_publish:peek() and lib.publish:peek()
	projects
		.new_repository(device, {
			name = name,
			parent = if parent ~= "" then parent else nil,
			empty = not new_readme:peek(),
			publish = publish,
			private = new_private:peek(),
		})
		:after(function(made: any)
			M.busy:set(false)
			if type(made) ~= "table" or type(made.repository) ~= "table" then
				return
			end
			lib.add(made.repository)
			if type(made.warning) == "string" and made.warning ~= "" then
				toast.show { title = locale.s("shell.repositories.made_but", { name = made.repository.name }), body = made.warning, tone = "warning" }
			end
			done(made.repository)
		end, function(err: any)
			M.busy:set(false)
			new_failed:set(M.says(err))
		end)
end

-- ── Clone ───────────────────────────────────────────────────────────────

local clone_url = ui.state("shell_repositories_clone_url", "")
local clone_parent = ui.state("shell_repositories_clone_parent", "")
local clone_failed = ui.state("shell_repositories_clone_failed", nil :: string?)

function M.reset_clone(device: string)
	M.library(device).load()
	ui.batch(function()
		clone_url:set("")
		clone_parent:set("")
		clone_failed:set(nil)
		M.busy:set(false)
	end)
end

-- The folder an address clones into (as `git clone` names it).
local function clone_folder(url: string): string
	local t = string.match(url, "^%s*(.-)%s*$") or ""
	t = string.gsub(t, "/+$", "")
	t = string.gsub(t, "%.git$", "")
	return string.match(t, "([^/:\\]+)$") or ""
end

function M.CloneForm(device: () -> string): Node
	local function lib(): Library
		return M.library(device())
	end
	local function where(): string
		local typed = string.match(clone_parent:get(), "^%s*(.-)%s*$") or ""
		return if typed ~= "" then typed else M.tilde(lib().parent:get(), lib().home:get())
	end
	return ui.column {
		id = "clone-repository",
		w_full = true,
		gap = "$space.4",
		ds.TextField {
			value = clone_url,
			label = locale.t("shell.repositories.clone_form.url"),
			placeholder = "https://github.com/owner/repository",
			autofocus = true,
			inputmode = "url",
			w_full = true,
			hint = function(): string
				local folder = clone_folder(clone_url:get())
				if folder == "" then
					return locale.s("shell.repositories.clone_form.hint")
				end
				local base = where()
				if string.sub(base, -1) ~= "/" then
					base ..= "/"
				end
				return locale.s("shell.repositories.clone_form.into", { path = base .. folder })
			end,
			error = function(): string?
				return clone_failed:get()
			end,
		},
		ds.TextField {
			value = clone_parent,
			label = locale.t("shell.repositories.new_form.location"),
			placeholder = function(): string
				return M.tilde(lib().parent:get(), lib().home:get())
			end,
			icon = "folder",
			w_full = true,
			size = "sm",
		},
	}
end

function M.submit_clone(device: string, done: (repository: Repository) -> ())
	if M.busy:peek() then
		return
	end
	local url = string.match(clone_url:peek(), "^%s*(.-)%s*$") or ""
	if url == "" then
		clone_failed:set(locale.s("shell.repositories.clone_form.no_url"))
		return
	end
	local lib = M.library(device)
	local typed = string.match(clone_parent:peek(), "^%s*(.-)%s*$") or ""
	local parent = if typed ~= "" then typed else lib.parent:peek()
	clone_failed:set(nil)
	M.busy:set(true)
	projects.clone(device, url, parent):after(function(repo: any)
		M.busy:set(false)
		if type(repo) ~= "table" then
			return
		end
		local made: Repository = { path = repo.path, name = repo.name, kept = true }
		lib.add(made)
		lib.load(true)
		done(made)
	end, function(err: any)
		M.busy:set(false)
		clone_failed:set(M.says(err))
	end)
end

-- ── on their own ────────────────────────────────────────────────────────

local open = ui.state("shell_repositories_dialog_open", false)
local mode = ui.state("shell_repositories_dialog_mode", "new")
local on_device = ui.state("shell_repositories_dialog_device", "")
local after: ((repository: Repository) -> ())? = nil

function M.open_new(device: string, done: ((repository: Repository) -> ())?)
	M.reset_new(device)
	after = done
	ui.batch(function()
		on_device:set(device)
		mode:set("new")
		open:set(true)
	end)
end

function M.open_clone(device: string, done: ((repository: Repository) -> ())?)
	M.reset_clone(device)
	after = done
	ui.batch(function()
		on_device:set(device)
		mode:set("clone")
		open:set(true)
	end)
end

local function device_name(id: string): string
	for _, d in sessions.devices:get() do
		if d.id == id then
			return d.name
		end
	end
	return id
end
M.device_name = device_name

function M.Dialog(): Node
	local function device(): string
		return on_device:get()
	end
	local function finish(repository: Repository)
		open:set(false)
		local then_ = after
		after = nil
		if then_ then
			then_(repository)
		end
	end
	return ds.Dialog {
		open = open,
		size = "md",
		title = function(): string
			return if mode:get() == "clone"
				then locale.s("shell.repositories.clone_title")
				else locale.s("shell.repositories.new_title")
		end,
		body = function(): string
			return locale.s(
				if mode:get() == "clone" then "shell.repositories.clone_description" else "shell.repositories.new_description",
				{ device = device_name(device()) }
			)
		end,
		hold = function(): boolean
			return M.busy:get()
		end,
		confirm = {
			label = function(): string
				return if mode:get() == "clone"
					then locale.s("shell.repositories.clone_form.go")
					else locale.s("shell.repositories.new_form.go")
			end,
			keep_open = true,
			busy = function(): boolean
				return M.busy:get()
			end,
			on_click = function()
				if mode:peek() == "clone" then
					M.submit_clone(device(), finish)
				else
					M.submit_new(device(), finish)
				end
			end,
		},
		ui.switch(function(): string
			return mode:get()
		end, {
			new = M.NewForm(device),
			clone = M.CloneForm(device),
		}),
	}
end

return M

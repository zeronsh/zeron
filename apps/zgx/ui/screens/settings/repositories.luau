-- Settings → Repositories: the git repositories on your computers, one
-- computer at a time (this one first): those found looking through its home,
-- and those added, cloned or made through Zeron. Newest worked-in first,
-- each with where it is, its branch and its GitHub repository, and the
-- projects it's in. From here: a new repository or a clone (New repository,
-- Clone), a chat in one (its project, else it added as one), adding it to a
-- project of repositories on that computer, its folder shown (this
-- computer's), its path copied, and forgetting it (never listed again;
-- nothing on disk changes, which the toast says).
--
-- A computer too old to list them, or offline, says so; Cloud's machines
-- aren't listed (a Cloud project is its GitHub repository).

local ds = require("@ds")
local Menu = require("@ds/menu")
local locale = require("@ds/locale")
local toast = require("@ds/toast")
local sessions = require("@account/sessions")
local projects = require("@account/projects")
local search = require("@app/screens/settings/search")
local files = require("@app/screens/files/api")
local repositories = require("@app/screens/shell/repositories")

local M = {}

M.ID = "repositories"
M.TITLE = "Repositories"

search.index(M.ID, ui.t("settings.repositories.title"), ui.t("settings.repositories.terms"), {
	{ key = "repositories", title = ui.t("settings.repositories.new"), terms = ui.t("settings.repositories.new_terms") },
	{ key = "repositories", title = ui.t("settings.repositories.clone"), terms = ui.t("settings.repositories.clone_terms") },
})

local query = ui.state("settings_repositories_query", "")
local chosen_device = ui.state("settings_repositories_device", "")

-- Your computers (never Cloud's machines), this one first.
local function computers(): { sessions.Device }
	local out = {}
	local here = sessions.here:get()
	for _, d in sessions.computers(sessions.devices:get()) do
		if d.here or d.id == here then
			table.insert(out, 1, d)
		else
			table.insert(out, d)
		end
	end
	return out
end

local function device(): string
	local id = chosen_device:get()
	for _, d in computers() do
		if d.id == id then
			return id
		end
	end
	local first = computers()[1]
	return if first then first.id else ""
end

-- The computer shown: the one chosen, while it's there; else this one.
ui.effect(function()
	local d = device()
	if d ~= chosen_device:peek() then
		chosen_device:set(d)
	end
end)
ui.effect(function()
	local d = device()
	if d ~= "" and projects.composes(d) then
		repositories.library(d).load()
	end
end)

local function lib(): repositories.Library
	return repositories.library(device())
end

-- The projects on the device a repository is: its own folder's, or one
-- holding it (by path; included ones are known only to their project).
local function projects_of(path: string): { sessions.Project }
	local out = {}
	for _, p in sessions.projects:get() do
		if p.device == device() and (p.path == path or string.sub(path, 1, #p.path + 1) == p.path .. "/") then
			table.insert(out, p)
		end
	end
	return out
end

local function shown(): { repositories.Repository }
	return repositories.matching(lib().list:get(), query:get())
end

-- In a chat: its project's (the one that is it), else it added as one.
local function chat_in(r: repositories.Repository)
	for _, p in sessions.projects:peek() do
		if p.device == device() and p.path == r.path then
			require("@app/screens/shell/route").new_chat(p.id)
			return
		end
	end
	require("@app/screens/shell/add_project").add(device(), r.path, true)
end

local function forget(r: repositories.Repository)
	local on = device()
	projects.forget_repository(on, r.path):after(function()
		local l = repositories.library(on)
		local next = {}
		for _, it in l.list:peek() do
			if it.path ~= r.path then
				table.insert(next, it)
			end
		end
		l.list:set(next)
		toast.show { title = locale.s("settings.repositories.forgotten", { name = r.name }), body = locale.s("settings.repositories.forgotten_body"), tone = "neutral" }
	end, function(err: any)
		toast.show { title = locale.s("settings.repositories.couldnt_forget"), body = repositories.says(err), tone = "danger" }
	end)
end

-- The projects of repositories on the device another can be added to.
local function hosts(): { sessions.Project }
	local out = {}
	for _, p in sessions.projects:get() do
		if p.device == device() and not p.git and not p.github then
			table.insert(out, p)
		end
	end
	return out
end

local function add_to(p: sessions.Project, r: repositories.Repository)
	projects.repositories(p.device, p.path):after(function(found: any)
		local members = {}
		for _, it in (if type(found) == "table" then found.list else {}) or {} do
			if it.location then
				table.insert(members, { path = it.location, name = it.path })
			end
		end
		table.insert(members, { path = r.path })
		projects.set_repositories(p.device, p.path, members):after(function()
			toast.show { title = locale.s("settings.repositories.added_to", { name = r.name, project = p.name }), tone = "success" }
		end, function(err: any)
			toast.show { title = locale.s("settings.repositories.couldnt_add"), body = repositories.says(err), tone = "danger" }
		end)
	end, function(err: any)
		toast.show { title = locale.s("settings.repositories.couldnt_add"), body = repositories.says(err), tone = "danger" }
	end)
end

local function row(r: repositories.Repository, _i: number, live: State<repositories.Repository>): Node
	local menu_open = ui.use_state(false)
	local more = ds.IconButton {
		icon = "more-horizontal",
		label = locale.s("settings.repositories.more_for", { name = r.name }),
		size = "md",
		tone = "quiet",
		on_click = function()
			menu_open:set(not menu_open:peek())
		end,
	}
	local function now(): repositories.Repository
		return live:get()
	end
	local function description(): string
		local it = now()
		local parts = { repositories.tilde(it.path, lib().home:get()) }
		if it.empty then
			table.insert(parts, locale.s("shell.repositories.no_commits"))
		elseif it.branch then
			table.insert(parts, it.branch)
		end
		if it.github then
			table.insert(parts, it.github)
		end
		return table.concat(parts, " · ")
	end
	local function in_projects(): string
		local names = {}
		for _, p in projects_of(now().path) do
			table.insert(names, p.name)
		end
		return if #names > 0 then locale.s("settings.repositories.in_projects", { names = table.concat(names, ", ") }) else ""
	end
	local function when(): string
		local t = now().touched
		return if t then locale.ago_short(os.time() - t / 1000) else ""
	end
	local add_rows = ui.each(ui.computed(hosts), function(p: sessions.Project): Node
		return Menu.Row {
			icon = "folder-with-files",
			label = p.name,
			on_click = function()
				menu_open:set(false)
				add_to(p, now())
			end,
		}
	end, {
		key = function(p: sessions.Project): string
			return p.id
		end,
		w_full = true,
	})
	local menu = ui.menu(Menu.panel {
		open = menu_open,
		anchor = more,
		placement = { side = "bottom", align = "end", offset = 4 },
		w = 240,
		Menu.Group {
			Menu.Row {
				icon = "chat-add",
				label = locale.s("settings.repositories.chat"),
				on_click = function()
					menu_open:set(false)
					chat_in(now())
				end,
			},
			ui.show(function(): boolean
				return device() == sessions.here:get()
			end, Menu.Row {
				icon = "folder",
				label = locale.s("settings.repositories.show"),
				on_click = function()
					menu_open:set(false)
					files.reveal({ device = device(), root = now().path }, ""):after(function() end, function(err: any)
						toast.show { title = locale.s("settings.repositories.couldnt_show"), body = repositories.says(err), tone = "danger" }
					end)
				end,
			}),
			Menu.Row {
				icon = "copy",
				label = locale.s("settings.repositories.copy_path"),
				on_click = function()
					menu_open:set(false)
					ui.clipboard.write(now().path)
				end,
			},
		},
		ui.show(function(): boolean
			return #hosts() > 0
		end, ui.column {
			w_full = true,
			Menu.Rule {},
			Menu.Group { heading = locale.s("settings.repositories.add_to"), add_rows },
		}),
		Menu.Rule {},
		Menu.Group {
			Menu.Row {
				icon = "eye-closed",
				label = locale.s("settings.repositories.forget"),
				danger = true,
				on_click = function()
					menu_open:set(false)
					forget(now())
				end,
			},
		},
	})
	return ds.Settings.Row {
		label = function(): string
			return now().name
		end,
		description = description,
		detail = function(): string?
			local s = in_projects()
			return if s ~= "" then s else nil
		end,
		truncate = true,
		lead = ui.div {
			w = 32,
			h = 32,
			shrink = 0,
			rounded = "$radius.md",
			bg = "$surface.sunken",
			border = 1,
			border_color = "$border.subtle",
			items = "center",
			justify = "center",
			color = "$text.secondary",
			ds.Icon { name = "git-branch", size = 16 },
		},
		control = ui.row {
			gap = "$space.2",
			items = "center",
			ui.text { when, text_size = "$font.size.xs", color = "$text.tertiary", tabular = true },
			-- (The menu draws its trigger, the ⋯, where it stands.)
			menu,
		},
	}
end

local function kind(): string
	local d = device()
	if d == "" then
		return "no_device"
	end
	if not projects.composes(d) then
		for _, it in computers() do
			if it.id == d and it.online == false then
				return "offline"
			end
		end
		return "old"
	end
	local l = lib()
	if #l.list:get() == 0 then
		if l.loading:get() then
			return "loading"
		elseif l.failed:get() then
			return "failed"
		end
		return "none"
	end
	return if #shown() == 0 then "nothing_found" else "list"
end

local function body(k: string): Node
	local function name(): string
		return repositories.device_name(device())
	end
	if k == "loading" then
		return ui.column {
			w_full = true,
			label = ui.t("common.loading"),
			live = "polite",
			ds.Settings.Group {
				ds.SkeletonRow { title = 90, line = 240, trail = 28 },
				ds.SkeletonRow { title = 130, line = 200, trail = 28 },
				ds.SkeletonRow { title = 110, line = 260, trail = 28 },
				ds.SkeletonRow { title = 70, line = 180, trail = 28 },
			},
		}
	elseif k == "failed" then
		return ds.EmptyState {
			icon = "wifi-off",
			size = "sm",
			title = locale.t("shell.repositories.failed.title"),
			body = function(): string
				return lib().failed:get() or ""
			end,
			action = {
				label = locale.t("shell.repositories.failed.retry"),
				on_click = function()
					lib().load(true)
				end,
			},
		}
	elseif k == "offline" then
		return ds.EmptyState {
			icon = "wifi-off",
			size = "sm",
			title = function(): string
				return locale.s("shell.project_repositories.offline", { device = name() })
			end,
			body = locale.t("settings.repositories.offline_body"),
		}
	elseif k == "old" then
		return ds.EmptyState {
			icon = "restart",
			size = "sm",
			title = function(): string
				return locale.s("shell.project_repositories.old", { device = name() })
			end,
			body = function(): string
				return locale.s("settings.repositories.old_body", { device = name() })
			end,
		}
	elseif k == "none" then
		return ds.EmptyState {
			icon = "git-branch",
			size = "sm",
			title = locale.t("shell.repositories.none.title"),
			body = locale.t("settings.repositories.none_body"),
			action = {
				label = locale.t("shell.repositories.new"),
				on_click = function()
					repositories.open_new(device())
				end,
			},
			secondary = {
				label = locale.t("shell.repositories.clone"),
				on_click = function()
					repositories.open_clone(device())
				end,
			},
		}
	elseif k == "nothing_found" then
		return ds.EmptyState {
			icon = "search",
			size = "sm",
			title = function(): string
				return locale.s("shell.repositories.unmatched", { query = query:get() })
			end,
		}
	elseif k == "no_device" then
		return ui.div {}
	end
	return ui.column {
		w_full = true,
		gap = "$space.2",
		ds.Settings.Each(ui.computed(shown), row, {
			key = function(r: repositories.Repository): string
				return r.path
			end,
		}),
		ui.text {
			function(): string
				return locale.s("settings.repositories.footer", { n = #lib().list:get() })
			end,
			px = ds.Settings.inset(),
			text_size = "$font.size.xs",
			line_height = "$font.line.sm",
			color = "$text.tertiary",
			wrap = true,
		},
	}
end

function M.view(): Node
	-- (Several computers: one at a time, chosen here; the options made
	-- again as computers come and go.)
	local picker = ui.presence(function(): string?
		local ids = {}
		for _, d in computers() do
			table.insert(ids, `{d.id}\0{d.name}`)
		end
		return if #ids > 1 then table.concat(ids, "\1") else nil
	end, function(): Node
		local options = {}
		for _, d in computers() do
			table.insert(options, { value = d.id, label = d.name })
		end
		return ui.div {
			px = ds.Settings.inset(),
			ds.Segmented {
				value = chosen_device,
				id = "settings-repositories-device",
				label = ui.t("settings.repositories.device"),
				options = options,
			},
		}
	end)
	return ui.column {
		w_full = true,
		items = "center",
		ds.Settings.Page {
			title = ui.t("settings.repositories.title"),
			description = ui.t("settings.repositories.description"),
			trail = ui.row {
				gap = "$space.2",
				items = "center",
				ds.SearchField {
					value = query,
					placeholder = ui.t("settings.repositories.find") :: any,
					size = "sm",
					w = 180,
					name = ui.t("settings.repositories.find") :: any,
				},
				ds.IconButton {
					icon = "refresh",
					label = locale.s("settings.repositories.refresh"),
					size = "sm",
					on_click = function()
						if projects.composes(device()) then
							lib().load(true)
						end
					end,
				},
				ds.Button {
					ui.t("shell.repositories.clone_short"),
					id = "settings-repositories-clone",
					size = "sm",
					tone = "secondary",
					on_click = function()
						repositories.open_clone(device())
					end,
				},
				ds.Button {
					ui.t("shell.repositories.new"),
					id = "settings-repositories-new",
					size = "sm",
					on_click = function()
						repositories.open_new(device())
					end,
				},
			},
			search.anchor(
				"repositories",
				ui.column {
					w_full = true,
					gap = "$space.4",
					picker,
					ui.presence(kind, body, {
						w_full = true,
						enter = { opacity = { 0, 1 }, duration = 120 },
						exit = { opacity = 0, duration = 60 },
						mode = "wait",
					}),
				}
			),
		},
	}
end

return M

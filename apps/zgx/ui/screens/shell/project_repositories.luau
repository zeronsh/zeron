-- Edit project's Repositories: what a project of repositories holds, and
-- changing it. Its folder's own repositories are listed as they are (they're
-- the folder's); those it includes from elsewhere on its device can be taken
-- out, and more added from anywhere there (the device's repositories, a new
-- one, or one cloned). Nothing changes until Save; then the host makes and
-- removes the links (`SetProjectRepositories`), never the repositories.
--
--   REPOSITORIES · 3                          [+ Add]
--   ┌──────────────────────────────────────────────┐
--   │ ▣ api      ~/code/api            ⑂ main    × │
--   │ ▣ web      ~/work/web-app        ⑂ develop × │
--   │ ▣ ui       In the project's folder  ⑂ main   │
--   │ ⚠ docs     Not found at ~/code/docs        × │
--   └──────────────────────────────────────────────┘
--   New worktrees check out the ones you choose, on one branch.
--
-- A project that is one repository says so, with the way to a project of
-- several: New project from repositories, it chosen. A Cloud project, or a
-- host too old to do this, says so instead.
--
--   tab.reset(project)        as the editor opens on it
--   tab.Panel(project)        the tab's content (`project`: a function)
--   tab.view()                "list", or a step of adding ("add", "new",
--                             "clone"): the dialog's confirm is that step's
--   tab.confirm_label()       the confirm's words in a step
--   tab.confirm()             the confirm in a step
--   tab.changed()             whether Save has something to do here
--   tab.save(project, done)   saved: `done(err?)`
--   tab.count()               how many repositories it holds (the tab's)

local ds = require("@ds")
local Icon = require("@ds/icon")
local FadeText = require("@ds/fade_text")
local Skeleton = require("@ds/skeleton")
local IconButton = require("@ds/icon_button").IconButton
local control = require("@ds/control")
local locale = require("@ds/locale")
local sessions = require("@account/sessions")
local projects = require("@account/projects")
local repositories = require("@app/screens/shell/repositories")

local M = {}

type Listed = { path: string, branch: string?, empty: boolean?, location: string?, missing: boolean?, github: string? }
type Listing = { repo: boolean, truncated: boolean?, list: { Listed } }
-- One repository the project includes from elsewhere, as it will be saved:
-- where it is, the name it goes by (nil: its folder's), and what's known of
-- it to show.
type Member = { location: string, name: string?, shown: string, branch: string?, empty: boolean?, missing: boolean?, github: string?, added: boolean? }

local listing = ui.state("project_repos_listing", nil :: Listing?)
local loading = ui.state("project_repos_loading", false)
local failed = ui.state("project_repos_failed", nil :: string?)
-- The members as they'll be saved; nil while unchanged.
local staged = ui.state("project_repos_staged", nil :: { Member }?)
local step = ui.state("project_repos_step", "list")
local add_chosen = ui.state("project_repos_add_chosen", {} :: { [string]: boolean })
local add_query = ui.state("project_repos_add_query", "")
local project_id = ui.state("project_repos_project", "")
local asked = 0

local function own_and_members(l: Listing?): ({ Listed }, { Member })
	local own, members = {}, {}
	for _, r in (if l then l.list else {}) do
		if r.location then
			table.insert(members, {
				location = r.location,
				name = r.path,
				shown = r.path,
				branch = r.branch,
				empty = r.empty,
				missing = r.missing,
				github = r.github,
			})
		else
			table.insert(own, r)
		end
	end
	return own, members
end

local function members(): { Member }
	local s = staged:get()
	if s then
		return s
	end
	local _, m = own_and_members(listing:get())
	return m
end

local function own(): { Listed }
	local o = own_and_members(listing:get())
	return o
end

function M.count(): number
	return #own() + #members()
end

-- The tab's count: how many it holds, once known ("" before; one for a
-- project that is one repository, or a Cloud project).
function M.badge(): string
	local l = listing:get()
	if l and l.repo then
		return "1"
	elseif l then
		return tostring(M.count())
	end
	return ""
end

function M.view(): string
	return step:get()
end

function M.changed(): boolean
	return staged:get() ~= nil
end

function M.reset(p: any)
	asked += 1
	local mine = asked
	ui.batch(function()
		project_id:set(p.id)
		listing:set(nil)
		failed:set(nil)
		staged:set(nil)
		step:set("list")
		add_chosen:set({})
		add_query:set("")
		loading:set(false)
	end)
	-- (A Cloud project's is its GitHub repository; a host that can't, none.)
	if p.github and string.sub(p.device, 1, 6) == "cloud-" then
		return
	end
	if not projects.composes(p.device) then
		return
	end
	loading:set(true)
	repositories.library(p.device).load()
	projects.repositories(p.device, p.path):after(function(found: any)
		if mine ~= asked then
			return
		end
		ui.batch(function()
			loading:set(false)
			listing:set(if type(found) == "table" then found else nil)
		end)
	end, function(err: any)
		if mine ~= asked then
			return
		end
		ui.batch(function()
			loading:set(false)
			failed:set(repositories.says(err))
		end)
	end)
end

local function stage(next: { Member })
	staged:set(next)
end

local function remove(location: string)
	local next = {}
	for _, m in members() do
		if m.location ~= location then
			table.insert(next, m)
		end
	end
	stage(next)
end

-- Every repository the project holds, by where it really is (none offered
-- again in Add).
local function held(p: any): { [string]: boolean }
	local out: { [string]: boolean } = {}
	local sep = if string.find(p.path, "\\", 1, true) then "\\" else "/"
	for _, r in own() do
		out[p.path .. sep .. string.gsub(r.path, "/", sep)] = true
	end
	for _, m in members() do
		out[m.location] = true
	end
	-- (Nor the project's own folder, when it's one.)
	out[p.path] = true
	return out
end

-- Those chosen in Add, added to the members.
local function add_chosen_ones(p: any)
	local library = repositories.library(p.device)
	local known: { [string]: any } = {}
	for _, r in library.list:peek() do
		known[r.path] = r
	end
	local next = table.clone(members())
	local have = held(p)
	-- (In the device list's order: newest worked-in first.)
	for _, r in library.list:peek() do
		if add_chosen:peek()[r.path] and not have[r.path] then
			table.insert(next, { location = r.path, shown = r.name, branch = r.branch, empty = r.empty, github = r.github, added = true })
		end
	end
	for path in add_chosen:peek() do
		if not known[path] and not have[path] then
			local name = string.match(path, "([^/\\]+)$") or path
			table.insert(next, { location = path, shown = name, added = true })
		end
	end
	ui.batch(function()
		stage(next)
		add_chosen:set({})
		add_query:set("")
		step:set("list")
	end)
end

local function chosen_n(): number
	local n = 0
	for _ in add_chosen:get() do
		n += 1
	end
	return n
end

function M.confirm_label(): string
	local s = step:get()
	if s == "new" then
		return locale.s("shell.repositories.new_form.go")
	elseif s == "clone" then
		return locale.s("shell.repositories.clone_form.go")
	end
	local n = chosen_n()
	return if n == 0 then locale.s("shell.project_repositories.add") else locale.s("shell.project_repositories.add_n", { n = n })
end

function M.confirm(p: any)
	local s = step:peek()
	if s == "new" then
		repositories.submit_new(p.device, function(r)
			add_chosen:set_key({ r.path }, true)
			step:set("add")
		end)
	elseif s == "clone" then
		repositories.submit_clone(p.device, function(r)
			add_chosen:set_key({ r.path }, true)
			step:set("add")
		end)
	elseif s == "add" then
		if chosen_n() > 0 then
			add_chosen_ones(p)
		end
	end
end

function M.confirm_disabled(): boolean
	return step:get() == "add" and chosen_n() == 0
end

function M.save(p: any, done: (err: any?) -> ())
	local s = staged:peek()
	if not s then
		done(nil)
		return
	end
	local asked_for = {}
	for _, m in s do
		table.insert(asked_for, { path = m.location, name = if m.added then nil else m.name })
	end
	projects.set_repositories(p.device, p.path, asked_for):after(function(found: any)
		ui.batch(function()
			if type(found) == "table" then
				listing:set(found)
			end
			staged:set(nil)
		end)
		done(nil)
	end, function(err: any)
		done(repositories.says(err))
	end)
end

-- ── the list ────────────────────────────────────────────────────────────

local ROW_H = 46

local function row(lead: Node, title: () -> string, under: () -> string, under_tone: () -> string, trail: { Node }): Node
	return ui.row {
		w_full = true,
		h = ROW_H,
		px = "$space.3",
		gap = "$space.3",
		items = "center",
		class = "project-repository",
		lead,
		ui.column {
			grow = 1,
			min_w = 0,
			gap = 1,
			FadeText { title, text_size = "$font.size.md", weight = "$font.weight.medium", color = "$text.primary" },
			FadeText { under, text_size = "$font.size.xs", color = under_tone, literal = true },
		},
		table.unpack(trail),
	}
end

local function glyph(name: Icon.Name, ink: string?): Node
	return ui.div {
		w = 28,
		h = 28,
		shrink = 0,
		rounded = "$radius.md",
		bg = "$surface.raised",
		border = 1,
		border_color = "$border.subtle",
		items = "center",
		justify = "center",
		color = ink or "$text.secondary",
		Icon { name = name, size = 14 },
	}
end

local function branch_of(r: () -> any): Node
	return ui.row {
		shrink = 0,
		max_w = 120,
		gap = "$space.1",
		items = "center",
		color = "$text.tertiary",
		ui.show(function(): boolean
			local it = r()
			return it.branch ~= nil and not it.empty and not it.missing
		end, Icon { name = "git-branch", size = 12 }),
		FadeText {
			function(): string
				local it = r()
				if it.missing then
					return ""
				elseif it.empty then
					return locale.s("shell.repositories.no_commits")
				end
				return it.branch or ""
			end,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		},
	}
end

local function list_view(p: () -> any): Node
	local function home(): string
		return repositories.library(p().device).home:get()
	end
	local own_rows = ui.each(ui.computed(own), function(r: Listed, _i: number, live: State<Listed>): Node
		local function now(): Listed
			return live:get()
		end
		return row(glyph("folder"), function(): string
			return now().path
		end, function(): string
			return locale.s("shell.project_repositories.in_folder")
		end, function(): string
			return "$text.tertiary"
		end, { branch_of(now), ui.div { w = 28, shrink = 0 } })
	end, {
		key = function(r: Listed): string
			return r.path
		end,
		w_full = true,
	})
	local member_rows = ui.each(ui.computed(members), function(m: Member, _i: number, live: State<Member>): Node
		local function now(): Member
			return live:get()
		end
		local location = m.location
		return row(
			ui.switch(function(): string
				return if now().missing then "missing" else "here"
			end, {
				missing = function(): Node
					return glyph("danger-triangle", "$warning")
				end,
				here = function(): Node
					return glyph("worktree")
				end,
			}),
			function(): string
				return now().shown
			end,
			function(): string
				local it = now()
				local where = repositories.tilde(it.location, home())
				if it.missing then
					return locale.s("shell.project_repositories.missing", { path = where })
				elseif it.added then
					return locale.s("shell.project_repositories.adding", { path = where })
				end
				return where
			end,
			function(): string
				local it = now()
				return if it.missing then "$warning" elseif it.added then "$accent" else "$text.tertiary"
			end,
			{
				branch_of(now),
				IconButton {
					icon = "close",
					size = "sm",
					tone = "quiet",
					label = locale.s("shell.project_repositories.remove", { name = m.shown }),
					tooltip = locale.s("shell.project_repositories.remove_tip"),
					on_click = function()
						remove(location)
					end,
				},
			}
		)
	end, {
		key = function(m: Member): string
			return m.location
		end,
		w_full = true,
	})

	local card = ui.column {
		id = "project-repositories-list",
		w_full = true,
		rounded = "$radius.lg",
		bg = "$surface.sunken",
		border = 1,
		border_color = "$border.subtle",
		overflow = "hidden",
		ui.scroll {
			w_full = true,
			overscroll = "contain",
			-- As tall as its rows, up to six of them.
			h = function(): number
				return ROW_H * math.min(math.max(M.count(), 1), 6)
			end :: any,
			ui.column { w_full = true, member_rows, own_rows },
		},
	}
	local empty = ds.EmptyState {
		icon = "worktree",
		size = "sm",
		title = locale.t("shell.project_repositories.none"),
		body = locale.t("shell.project_repositories.none_body"),
		action = {
			label = locale.t("shell.project_repositories.add"),
			on_click = function()
				step:set("add")
			end,
		},
	}
	return ui.column {
		w_full = true,
		gap = "$space.3",
		ui.row {
			w_full = true,
			items = "center",
			gap = "$space.2",
			ui.text {
				function(): string
					return locale.s("shell.project_repositories.heading", { n = M.count() })
				end,
				id = "project-repositories-count",
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.secondary",
				grow = 1,
			},
			ds.Button {
				ui.row { gap = "$space.1", items = "center", Icon { name = "plus", size = 12 }, ui.text { locale.t("shell.project_repositories.add") } } :: any,
				id = "project-repositories-add",
				tone = "secondary",
				size = "sm",
				label = locale.t("shell.project_repositories.add"),
				on_click = function()
					add_chosen:set({})
					add_query:set("")
					step:set("add")
				end,
			},
		},
		ui.switch(function(): string
			return if M.count() == 0 then "empty" else "list"
		end, { empty = empty, list = card }),
		ui.text {
			function(): string
				return if M.changed()
					then locale.s("shell.project_repositories.save_hint")
					else locale.s("shell.project_repositories.hint")
			end,
			text_size = "$font.size.xs",
			line_height = "$font.line.sm",
			color = "$text.tertiary",
			wrap = true,
		},
	}
end

local function back(): Node
	return ui.button {
		id = "project-repositories-back",
		label = locale.t("shell.project_repositories.back") :: any,
		h = 24,
		px = "$space.1",
		ml = -4,
		rounded = "$radius.sm",
		gap = "$space.1",
		items = "center",
		text_size = "$font.size.sm",
		color = "$text.secondary",
		bg = "#00000000",
		hover = { bg = "$hover", color = "$text.primary" },
		focus = control.ring,
		on_click = function()
			if repositories.busy:peek() then
				return
			end
			step:set(if step:peek() == "add" then "list" else "add")
		end,
		Icon { name = "alt-arrow-left", size = 14 },
		ui.text {
			function(): string
				return if step:get() == "add"
					then locale.s("shell.project_repositories.back")
					else locale.s("shell.project_repositories.back_to_add")
			end,
		},
	}
end

-- A note in the tab's place (a Cloud project, a host that can't).
local function note(icon: Icon.Name, title: () -> string, body: () -> string, action: any?): Node
	return ds.EmptyState { icon = icon, size = "sm", title = title, body = body, action = action }
end

function M.Panel(p: () -> any): Node
	local function state(): string
		local it = p()
		if not it then
			return "none"
		end
		if it.github and string.sub(it.device, 1, 6) == "cloud-" then
			return "cloud"
		end
		if not projects.composes(it.device) then
			for _, d in sessions.devices:get() do
				if d.id == it.device and d.online == false then
					return "offline"
				end
			end
			return "old"
		end
		local l = listing:get()
		if not l then
			return if failed:get() then "failed" elseif loading:get() then "loading" else "none"
		end
		if l.repo then
			return "repo"
		end
		return step:get()
	end
	local function device_name(): string
		local it = p()
		return if it then repositories.device_name(it.device) else ""
	end
	return ui.column {
		id = "project-repositories",
		w_full = true,
		ui.switch(state, {
			none = ui.div {},
			loading = ui.column {
				w_full = true,
				gap = "$space.3",
				Skeleton.Bar { w = 120, h = 12 },
				Skeleton.Row { lead = 28, title = 80, line = 160, trail = 48 },
				Skeleton.Row { lead = 28, title = 110, line = 200, trail = 48 },
			},
			failed = note("wifi-off", function(): string
				return locale.s("shell.project_repositories.failed")
			end, function(): string
				return failed:get() or ""
			end),
			cloud = note("cloud", function(): string
				return locale.s("shell.project_repositories.cloud")
			end, function(): string
				local it = p()
				return locale.s("shell.project_repositories.cloud_body", { repo = if it then it.github or "" else "" })
			end),
			offline = note("wifi-off", function(): string
				return locale.s("shell.project_repositories.offline", { device = device_name() })
			end, function(): string
				return locale.s("shell.project_repositories.offline_body")
			end),
			old = note("restart", function(): string
				return locale.s("shell.project_repositories.old", { device = device_name() })
			end, function(): string
				return locale.s("shell.project_repositories.old_body", { device = device_name() })
			end),
			repo = note("git-branch", function(): string
				return locale.s("shell.project_repositories.one")
			end, function(): string
				return locale.s("shell.project_repositories.one_body")
			end, {
				label = locale.t("shell.project_repositories.combine"),
				on_click = function()
					local it = p()
					if not it then
						return
					end
					M.combine(it)
				end,
			}),
			list = list_view(p),
			add = ui.column {
				w_full = true,
				gap = "$space.3",
				back(),
				repositories.Picker {
					id = "project-repositories-picker",
					device = function(): string
						local it = p()
						return if it then it.device else ""
					end,
					chosen = add_chosen,
					query = add_query,
					rows = 5,
					hidden = function(): { [string]: boolean }
						local it = p()
						return if it then held(it) else {}
					end,
					on_new = function()
						local it = p()
						if it then
							repositories.reset_new(it.device, string.match(add_query:peek(), "^%s*(.-)%s*$"))
							step:set("new")
						end
					end,
					on_clone = function()
						local it = p()
						if it then
							repositories.reset_clone(it.device)
							step:set("clone")
						end
					end,
				},
			},
			new = ui.column {
				w_full = true,
				gap = "$space.3",
				back(),
				repositories.NewForm(function(): string
					local it = p()
					return if it then it.device else ""
				end),
			},
			clone = ui.column {
				w_full = true,
				gap = "$space.3",
				back(),
				repositories.CloneForm(function(): string
					local it = p()
					return if it then it.device else ""
				end),
			},
		}),
	}
end

-- A project that is one repository, with others: New project from
-- repositories, it chosen and its name kept for the new one's start.
M.on_combine = nil :: ((p: any) -> ())?
function M.combine(p: any)
	if M.on_combine then
		M.on_combine(p)
	end
	require("@app/screens/shell/compose_project").open(p.device, { chosen = { p.path } })
end

return M

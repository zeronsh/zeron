-- New project, as the shipping app adds one (crates/ui shell/spaces.rs
-- `AddSpaceFlow`): a palette, its steps a page each. Your devices; on one,
-- Home and its drives; then its folders, one inside another, each its own
-- page, "Add this folder" first. Every device is browsed through the engine,
-- this Mac's too (no folder dialog). Under Home, what's typed also finds
-- folders anywhere under it ("In Home", the engine's index). A path typed
-- (`~/code`, `/mnt`; ⌘L starts from the folder's) goes there. A git address
-- typed in a folder offers to clone it there. Added (`createSpace`, named after its folder), the
-- window lands in a new chat in it; a folder that's a project already just
-- opens it.
--
-- Cloud, while it's on, is the devices' last row (one for all of Cloud: its
-- sessions' machines are never devices here): its page lists the GitHub
-- repositories Cloud can open, by owner, newest push first, each saying if
-- it's private or a project on a device already; one picked is a Cloud
-- project (`createSpace` on Cloud's device, the repository named, as the
-- shipping app's `add_space_create_cloud`), or the one Cloud has already,
-- and the window lands in a new chat in it. Without GitHub, or without a
-- repository, the page says so and leads to Settings → Cloud. A folder's
-- search also offers those repositories (From GitHub), to clone there.
--
--   AddProject()            placed once in the window
--   add_project.open()      ⌘⇧N, the palette's and the project filter's
--                           "New project", the composer's
--   add_project.shown       whether it's open (a state)

local CommandPalette = require("@ds/command_palette")
local toast = require("@ds/toast")
local locale = require("@ds/locale")
local sessions = require("@account/sessions")
local projects = require("@account/projects")
local cloud = require("@account/cloud")
local route = require("@app/screens/shell/route")
local view = require("@app/screens/shell/view")

local M = {}

M.shown = view.add_project
function M.open()
	view.palette:set(false)
	M.shown:set(true)
end
-- Its steps as a page of ⌘K's own (its "New project": the palette slides
-- to it as to "New chat in…", Back returning), while ⌘K shows it.
M.hosted = ui.state("shell_add_project_hosted", false)
ui.effect(function()
	if not view.palette:get() and M.hosted:peek() then
		M.hosted:set(false)
	end
end)
-- Whether the flow is showing, in its own palette or in ⌘K's.
local function showing(): boolean
	return M.shown:get() or M.hosted:get()
end
local function showing_now(): boolean
	return M.shown:peek() or M.hosted:peek()
end

type Item = any
type Page = any

-- The engine's refusal, as the flow says it (production's own words).
local function says(err: any, path: string?): string
	local text = if type(err) == "string" then err elseif type(err) == "table" and err.message then tostring(err.message) else locale.s("shell.add_project.something_went_wrong")
	if string.find(text, "not connected", 1, true) or text == "offline" then
		return locale.s("shell.add_project.not_connected")
	elseif path and (string.find(text, "os error 2)", 1, true) or string.find(text, "os error 3)", 1, true)) then
		return locale.s("shell.add_project.no_folder_at", { path = path })
	elseif path and (string.find(text, "os error 20)", 1, true) or string.find(text, "os error 267)", 1, true)) then
		return locale.s("shell.add_project.not_a_folder", { path = path })
	end
	return text
end

-- The project added and waiting to arrive (the engine tells the sessions a
-- beat after it answers): the window lands in it once it's there.
local arriving = ui.state("shell_add_project_arriving", nil :: string?)
ui.effect(function()
	local id = arriving:get()
	if not id then
		return
	end
	for _, p in sessions.projects:get() do
		if p.id == id then
			arriving:set(nil)
			route.new_chat(id)
			return
		end
	end
end)

-- A folder as a project on `device`: the one there already, else made.
-- (`repo`: a Cloud project's repository; Cloud's same repository is the
-- same project.)
function M.add(device: string, path: string, git: boolean, repo: string?)
	for _, p in sessions.now:peek().projects do
		local same_repo = repo ~= nil and p.github ~= nil and string.lower(p.github :: string) == string.lower(repo :: string)
		if p.device == device and (p.path == path or same_repo) then
			route.new_chat(p.id)
			return
		end
	end
	projects.create(device, path, git, repo):after(function(id: any)
		if type(id) == "string" then
			arriving:set(id)
		end
	end, function(err: any)
		toast.show { title = locale.s("shell.add_project.couldnt_add"), body = says(err, path), tone = "danger" }
	end)
end

-- `name` inside `parent`, with the separator the path already uses.
local function join(parent: string, name: string): string
	local sep = if string.find(parent, "\\", 1, true) and not string.find(parent, "/", 1, true) then "\\" else "/"
	if string.sub(parent, -1) == sep then
		return parent .. name
	end
	return parent .. sep .. name
end

-- A git address (production's `git_url`): its owner/name, else nil.
local function git_url(text: string): string?
	local t = string.match(text, "^%s*(.-)%s*$") or ""
	local tail: string? = nil
	for _, scheme in { "https://", "http://", "ssh://", "git://" } do
		if string.sub(t, 1, #scheme) == scheme then
			tail = string.match(string.sub(t, #scheme + 1), "^[^/]+/(.+)$")
		end
	end
	if not tail then
		tail = string.match(t, "^[%w%.%-_]+@[%w%.%-]+:(.+)$")
	end
	if not tail then
		return nil
	end
	tail = string.gsub(tail, "%.git$", "")
	tail = string.gsub(tail, "/$", "")
	local owner, name = string.match(tail, "^([^/]+)/([^/]+)$")
	return if owner and name then `{owner}/{name}` else nil
end

-- A folder on `device` (nil `path`: its home), as a page: "Add this folder",
-- then the folders inside it. (`git`: whether it's a repository, as the
-- folder above listed it.)
local folder_page: (device: string, path: string?, title: string, git: boolean, home: boolean) -> Page

-- Searching under a device's home (Zeron's "In Home"): its folders anywhere
-- under it, through the engine's index. The first ask on Home starts the
-- index (an empty query), so the first real search is quick; closing New
-- project lets it go. Two letters or more, not a path; asked again while it
-- indexes (½ s, doubling to 2 s), each answer replacing the last.
local HOME_LEAST, HOME_RETRY, HOME_RETRY_MOST = 2, 500, 2000
local warmed: { [string]: boolean } = {}
local function warm(device: string)
	if not warmed[device] then
		warmed[device] = true
		projects.search_home(device, ""):after(function() end, function() end)
	end
end
ui.effect(function()
	if showing() then
		return
	end
	for device in warmed do
		projects.release_home(device)
	end
	table.clear(warmed)
end)
-- Whether what's typed is a path (absolute, home's, a drive's). (`~foo`
-- is a folder's name.)
local function typed_path(text: string): boolean
	return string.sub(text, 1, 1) == "/"
		or text == "~"
		or string.match(text, "^~[/\\]") ~= nil
		or string.match(text, "^%a:[\\/]") ~= nil
		or string.sub(text, 1, 2) == "\\\\"
end

-- Each device's home, as its listing said (for `~`).
local homes: { [string]: string } = {}
-- What's typed as the path it names on `device`: trimmed, its trailing
-- separator dropped (a root's kept), `~` its home (asked for, not yet
-- known). `done(path)`, or `done(nil, why)`.
local function resolve(device: string, text: string, done: (string?, any?) -> ())
	local t = string.match(text, "^%s*(.-)%s*$") or ""
	if not (t == "/" or string.match(t, "^%a:[\\/]$")) then
		t = (string.gsub(t, "[/\\]+$", ""))
	end
	if t ~= "~" and not string.match(t, "^~[/\\]") then
		done(if t == "" then nil else t)
		return
	end
	local function under(home: string)
		local rest = string.sub(t, 3)
		done(if rest == "" then home else join(home, rest))
	end
	if homes[device] then
		under(homes[device])
		return
	end
	projects.folders(device, nil):after(function(listing: any)
		if type(listing) == "table" and type(listing.path) == "string" then
			homes[device] = listing.path
			under(listing.path)
		else
			done(nil, "Device is not connected")
		end
	end, function(err: any)
		done(nil, err)
	end)
end
-- A path as typed: under its device's home, `~/…`.
local function tilde(device: string, path: string): string
	local home = homes[device]
	if home and path == home then
		return "~"
	elseif home and string.sub(path, 1, #home + 1) == home .. "/" then
		return "~" .. string.sub(path, #home + 1)
	end
	return path
end
-- Whether a listing is a repository's (a `.git` inside).
local function has_git(listing: any): boolean
	for _, e in (if type(listing) == "table" and type(listing.entries) == "table" then listing.entries else {}) do
		if e.name == ".git" then
			return true
		end
	end
	return false
end
-- A path as a key (no trailing separator).
local function key_of(path: string): string
	return (string.gsub(path, "[/\\]+$", ""))
end
-- A match's name, and where it lives (`~/…`), from its path under home.
local function home_place(relative: string): (string, string)
	local parent, name = string.match(relative, "^(.*)/([^/]+)$")
	if parent then
		return name, `~/{parent}`
	end
	return relative, "~"
end

-- How much of `name` `typed` starts, any case (production's
-- `completion_prefix_len`): whether it does.
local function starts(name: string, typed: string): boolean
	return string.lower(string.sub(name, 1, #typed)) == string.lower(typed)
end
-- The folder of `dirs` a name typed means (production's `segment_target`):
-- the one named so exactly, else in any case, else the one it starts, if
-- only one.
type Dir = { name: string, repo: boolean }
local function named(dirs: { Dir }, typed: string): Dir?
	for _, d in dirs do
		if d.name == typed then
			return d
		end
	end
	for _, d in dirs do
		if string.lower(d.name) == string.lower(typed) then
			return d
		end
	end
	local only: Dir? = nil
	for _, d in dirs do
		if starts(d.name, typed) then
			if only then
				return nil
			end
			only = d
		end
	end
	return only
end
-- The one a name typed completes to: named so, else the first it starts.
local function completing(dirs: { Dir }, typed: string): Dir?
	for _, d in dirs do
		if d.name == typed then
			return d
		end
	end
	for _, d in dirs do
		if starts(d.name, typed) then
			return d
		end
	end
	return nil
end
-- `text` split at its last separator: the folder (separator kept) and the
-- name after it.
local function leaf(text: string): (string, string)
	local folder, name = string.match(text, "^(.*[/\\])([^/\\]*)$")
	if folder then
		return folder, name
	end
	return "", text
end

-- A path typed: that folder, opened (↵) or added (⌘↵), wherever it is.
local function go_to(device: string, query: string): any
	local typed = string.match(query, "^%s*(.-)%s*$") or query
	local name = string.match((string.gsub(typed, "[/\\]+$", "")), "([^/\\]+)$") or typed
	return {
		id = `goto:{typed}`,
		label = typed,
		section = locale.s("shell.add_project.go_to"),
		icon = "folder",
		keywords = query,
		verb = locale.s("shell.add_project.open"),
		page = function(): Page
			return folder_page(device, typed, name, false, false)
		end,
		alt = {
			label = locale.s("shell.add_project.add_project"),
			run = function()
				resolve(device, typed, function(at: string?, why: any?)
					if not at then
						toast.show { title = locale.s("shell.add_project.couldnt_add"), body = says(why, typed), tone = "danger" }
						return
					end
					local found = at :: string
					-- (Listed first: there, a folder, and whether it's a repository.)
					projects.folders(device, found):after(function(listing: any)
						M.add(device, found, has_git(listing))
					end, function(err: any)
						toast.show { title = locale.s("shell.add_project.couldnt_add"), body = says(err, found), tone = "danger" }
					end)
				end)
			end,
		},
	}
end

-- What's typed that is a path ending in its separator (`~/code/`,
-- `/var/folders/`), as the page it goes to at once; a disk's root alone
-- (`/`, `C:\`) waits for more (it's how any path starts), its "Go to" row
-- there for ↵.
local function jump(device: string, query: string): Page?
	local q = string.match(query, "^%s*(.-)%s*$") or ""
	local last = string.sub(q, -1)
	if (last ~= "/" and last ~= "\\") or not typed_path(q) then
		return nil
	end
	if key_of(q) == "" or string.match(q, "^%a:[\\/]$") or string.sub(q, 1, 2) == "\\\\" and not string.find(string.sub(q, 3), "[/\\].") then
		return nil
	end
	local name = string.match(key_of(q), "([^/\\]+)$") or q
	return folder_page(device, q, name, false, string.sub(q, 1, 1) == "~")
end
-- A path typed, as the one row offered (a promise of it).
local function offer(row: any): any
	return {
		after = function(_, ok: (any) -> ())
			ok({ row })
		end,
	}
end
-- Rows already known, as a promise of them.
local function offer_all(rows: { any }): any
	return {
		after = function(_, ok: (any) -> ())
			ok(rows)
		end,
	}
end
-- How many of your GitHub repositories a folder's search offers to clone.
local CLONE_MATCHES = 5

folder_page = function(device: string, path: string?, title: string, git: boolean, home: boolean): Page
	local page: Page = {
		id = `folder:{device}:{path or "~"}`,
		title = title,
		placeholder = locale.s("shell.add_project.search_folders"),
		loading = { title = locale.s("shell.add_project.loading_folders") },
	}
	local here: string? = path
	-- What this page lists (itself and its folders), left out of Home's.
	local listed: { [string]: boolean } = {}
	-- Its folders, for a name typed (`name/`, Tab); and what else it holds.
	local dirs: { Dir } = {}
	local others: { Dir } = {}
	-- (A typed path's `~` asked for first.)
	local function list(ok: (any) -> (), fail: (any) -> ())
		if not path then
			projects.folders(device, nil):after(ok, fail)
			return
		end
		resolve(device, path, function(at: string?, why: any?)
			if at then
				projects.folders(device, at):after(ok, fail)
			else
				fail(why)
			end
		end)
	end
	page.load = function(done: ({ Item }) -> ())
		list(function(listing: any)
			here = if type(listing) == "table" and type(listing.path) == "string" then listing.path else path
			if here then
				listed[key_of(here :: string)] = true
			end
			if not path and here then
				homes[device] = here :: string
			end
			-- (Typed, it may be a repository nobody said.)
			git = git or has_git(listing)
			local out: { Item } = {}
			if here then
				local at = here :: string
				table.insert(out, {
					id = "add-here",
					label = locale.s("shell.add_project.add_this_folder"),
					section = locale.s("shell.add_project.add"),
					icon = "plus",
					hint = at,
					verb = locale.s("shell.add_project.add_project"),
					run = function()
						M.add(device, at, git)
					end,
				})
			end
			for _, e in (if type(listing) == "table" and type(listing.entries) == "table" then listing.entries else {}) do
				if not e.is_dir and type(e.name) == "string" then
					table.insert(others, { name = e.name, repo = false })
				end
				-- (Folders, the hidden ones left out.)
				if e.is_dir and type(e.name) == "string" and string.sub(e.name, 1, 1) ~= "." and here then
					local child = join(here :: string, e.name)
					local repo = e.is_repo == true
					listed[key_of(child)] = true
					table.insert(dirs, { name = e.name, repo = repo })
					table.insert(out, {
						id = `dir:{child}`,
						label = e.name,
						section = locale.s("shell.add_project.folders"),
						icon = if repo then "git-branch" else "folder",
						verb = locale.s("shell.add_project.open"),
						page = function(): Page
							return folder_page(device, child, e.name, repo, home)
						end,
						-- (⌘↵: this one added, as it stands.)
						alt = {
							label = locale.s("shell.add_project.add_project"),
							run = function()
								M.add(device, child, repo)
							end,
						},
					})
				end
			end
			done(out)
		end, function(err: any)
			page.empty = { title = says(err, path), hint = locale.s("shell.add_project.folder_failed_hint") }
			done({})
		end)
	end
	page.empty = { title = locale.s("shell.add_project.no_folders_match"), hint = locale.s("shell.add_project.no_folders_hint") }
	-- ⌘L: this folder's path, a separator after it, so a name typed goes
	-- inside.
	page.path = function(): string?
		if not here then
			return nil
		end
		local at = tilde(device, here :: string)
		local sep = if string.find(at, "\\", 1, true) and not string.find(at, "/", 1, true) then "\\" else "/"
		return if string.sub(at, -1) == sep then at else at .. sep
	end
	-- What's typed ending in a separator goes there at once: a folder's
	-- `name/` into it, a path (`~/code/`, `/mnt/`) to it; not here again
	-- (⌘L's own path).
	page.typed = function(query: string): Page?
		local q = string.match(query, "^%s*(.-)%s*$") or ""
		local last = string.sub(q, -1)
		if last ~= "/" and last ~= "\\" or not here then
			return nil
		end
		if typed_path(q) then
			local at = page.path()
			if at and (q == at or key_of(q) == key_of(here :: string)) then
				return nil
			end
			return jump(device, q)
		end
		local typed = string.sub(q, 1, -2)
		if typed == "" or string.find(typed, "[/\\]") then
			return nil
		end
		-- (Else an entry that isn't listed as a folder: a link to one is
		-- listed so, and opens; a file says it isn't one.)
		local d = named(dirs, typed) or named(others, typed)
		if not d then
			return nil
		end
		local dir = d :: Dir
		return folder_page(device, join(here :: string, dir.name), dir.name, dir.repo, home)
	end
	-- Tab: a folder's name completed (here's folders), its separator after,
	-- so it goes in.
	page.complete = function(query: string): string?
		local folder, name = leaf(query)
		if name == "" or name == "." or name == ".." then
			return nil
		end
		-- (A path's: only one in this folder.)
		if folder ~= "" then
			local at = page.path()
			if not (at and folder == at) and not (here and key_of(folder) == key_of(here :: string)) then
				return nil
			end
		end
		local d = completing(dirs, name)
		if not d then
			return nil
		end
		local sep = if string.sub(folder, -1) == "\\" then "\\" else "/"
		return folder .. (d :: Dir).name .. sep
	end
	-- Home's folders matching what's typed, each a page as the listed ones.
	local asked = 0
	local function in_home(query: string): any
		asked += 1
		local mine = asked
		warm(device)
		local q = string.match(query, "^%s*(.-)%s*$") or ""
		if (utf8.len(q) or 0) < HOME_LEAST or typed_path(q) then
			return nil
		end
		return {
			after = function(_, ok: (any) -> ())
				local wait = HOME_RETRY
				local function look()
					projects.search_home(device, q):after(function(found: any)
						-- (Typed past, or New project closed: not this one's now.)
						if mine ~= asked or not showing_now() then
							return
						end
						local out: { Item } = {}
						for _, m in (if type(found) == "table" and type(found.matches) == "table" then found.matches else {}) do
							if type(m.path) == "string" and not listed[key_of(m.path)] then
								local name, place = home_place(if type(m.relative) == "string" then m.relative else m.path)
								local at, repo = m.path :: string, m.is_repo == true
								table.insert(out, {
									id = `home:{at}`,
									label = name,
									section = locale.s("shell.add_project.in_home"),
									icon = "folder",
									hint = place,
									keywords = q,
									verb = locale.s("shell.add_project.open"),
									page = function(): Page
										return folder_page(device, at, name, repo, true)
									end,
									alt = {
										label = locale.s("shell.add_project.add_project"),
										run = function()
											M.add(device, at, repo)
										end,
									},
								})
							end
						end
						ok(out)
						if type(found) == "table" and found.indexing then
							ui.after(wait, look)
							wait = math.min(wait * 2, HOME_RETRY_MOST)
						end
					end, function()
						if mine == asked then
							ok({})
						end
					end)
				end
				look()
			end,
		}
	end
	-- A repository cloned into this folder, then added.
	local function clone_into(url: string, repo: string)
		local parent = here
		if not parent then
			return
		end
		toast.show { title = locale.s("shell.add_project.cloning", { repo = repo }) }
		projects.clone(device, url, parent :: string):after(function(cloned: any)
			if type(cloned) == "table" and type(cloned.path) == "string" then
				M.add(device, cloned.path, true)
			end
		end, function(err: any)
			toast.show { title = locale.s("shell.add_project.couldnt_clone", { repo = repo }), body = says(err), tone = "danger" }
		end)
	end
	-- Your GitHub repositories a name typed matches (Cloud's, GitHub
	-- connected), a few, newest push first, to clone here (production's From
	-- GitHub).
	local function from_github(query: string): { Item }
		local needle = string.lower(string.match(query, "^%s*(.-)%s*$") or "")
		if needle == "" or string.find(needle, "^[/~%.]") then
			return {}
		end
		local out: { Item } = {}
		for _, r in cloud.now:peek().repos or {} do
			if #out >= CLONE_MATCHES then
				break
			end
			if r.clone_url and string.find(string.lower(r.name), needle, 1, true) then
				local url, name = r.clone_url :: string, r.name
				table.insert(out, {
					id = `github:{name}`,
					label = name,
					section = locale.s("shell.add_project.from_github"),
					icon = "git-branch",
					keywords = query,
					hint = if r.private then locale.s("shell.add_project.private") else nil,
					verb = locale.s("shell.add_project.clone"),
					run = function()
						clone_into(url, name)
					end,
				})
			end
		end
		return out
	end
	-- A git address typed here: cloned into this folder, then added (the
	-- one thing offered); else, under Home, Home's folders; and your GitHub
	-- repositories it names, after them.
	page.search = function(query: string): any
		local repo = git_url(query)
		if not repo then
			if typed_path(string.match(query, "^%s*(.-)%s*$") or "") then
				asked += 1
				return offer(go_to(device, query))
			end
			local github = from_github(query)
			local found = if home then in_home(query) else nil
			if not found then
				return if #github > 0 then offer_all(github) else nil
			end
			-- (Told again as Home's come: theirs, then GitHub's.)
			return {
				after = function(_, ok: (any) -> ())
					if #github > 0 then
						ok(github)
					end
					found:after(function(list: any)
						local all = table.clone(list or {})
						for _, g in github do
							table.insert(all, g)
						end
						ok(all)
					end)
				end,
			}
		end
		asked += 1
		local url = string.match(query, "^%s*(.-)%s*$") or query
		return {
			after = function(_, ok: (any) -> ())
				ok({
					{
						id = `clone:{url}`,
						label = locale.s("shell.add_project.clone_here", { repo = repo }),
						section = locale.s("shell.add_project.clone_section"),
						icon = "git-branch",
						verb = locale.s("shell.add_project.clone"),
						run = function()
							clone_into(url, repo)
						end,
					},
				})
			end,
		}
	end
	return page
end

-- A device's places to start from: Home, then its drives.
local function locations_page(device: string, name: string): Page
	local page: Page = {
		id = `places:{device}`,
		title = name,
		placeholder = locale.s("shell.add_project.search_locations"),
		loading = { title = locale.s("shell.add_project.loading_locations") },
		empty = { title = locale.s("shell.add_project.no_locations"), hint = locale.s("shell.add_project.no_locations_hint") },
	}
	-- A path typed here too: anywhere on the device, not only under Home
	-- or a drive.
	page.search = function(query: string): any
		if typed_path(string.match(query, "^%s*(.-)%s*$") or "") then
			return offer(go_to(device, query))
		end
		return nil
	end
	page.typed = function(query: string): Page?
		return jump(device, query)
	end
	page.load = function(done: ({ Item }) -> ())
		local function home(): Item
			return {
				id = "home",
				label = locale.s("shell.add_project.home"),
				section = locale.s("shell.add_project.locations"),
				icon = "folder",
				verb = locale.s("shell.add_project.open"),
				page = function(): Page
					return folder_page(device, nil, locale.s("shell.add_project.home"), false, true)
				end,
			}
		end
		projects.drives(device):after(function(drives: any)
			local out: { Item } = { home() }
			for _, d in (if type(drives) == "table" then drives else {}) do
				if type(d.path) == "string" then
					local dname = if type(d.name) == "string" and d.name ~= "" then d.name else d.path
					table.insert(out, {
						id = `drive:{d.path}`,
						label = dname,
						section = locale.s("shell.add_project.locations"),
						icon = "folder-with-files",
						hint = d.path,
						verb = locale.s("shell.add_project.open"),
						page = function(): Page
							return folder_page(device, d.path, dname, false, false)
						end,
					})
				end
			end
			done(out)
		end, function()
			-- (Its drives unknown: Home alone, as production.)
			done({ home() })
		end)
	end
	return page
end

-- Cloud's page: the GitHub repositories Cloud can open, read afresh as it
-- opens, each owner's together (the owners by their newest push, theirs
-- newest first); one picked is a Cloud project. Without GitHub, or with no
-- repository yet, the way to Settings → Cloud; a read that fails says why.
local function cloud_page(): Page
	local page: Page = {
		id = "cloud-repos",
		title = locale.s("shell.add_project.cloud_title"),
		placeholder = locale.s("shell.add_project.search_repos"),
		loading = { title = locale.s("shell.add_project.loading_repos") },
		empty = { title = locale.s("shell.add_project.no_repos"), hint = locale.s("shell.add_project.no_repos_hint") },
	}
	local function to_settings(id: string, label: string, section: string): Item
		return {
			id = id,
			label = label,
			section = section,
			icon = "arrow-up-right",
			verb = locale.s("shell.add_project.open"),
			run = function()
				route.settings("cloud")
			end,
		}
	end
	page.load = function(done: ({ Item }) -> ())
		cloud.load_repos(function()
			local now = cloud.now:peek()
			local why = now.repos_failure
			if why == "github_not_connected" then
				done({ to_settings("connect-github", locale.s("shell.add_project.connect_github"), locale.s("shell.add_project.github_not_connected")) })
				return
			elseif why and not now.repos then
				page.empty = {
					title = if why == "offline" then locale.s("shell.add_project.repos_offline") else locale.s("shell.add_project.repos_failed"),
					hint = locale.s("shell.add_project.repos_failed_hint"),
				}
				done({})
				return
			end
			local repos = table.clone(now.repos or {})
			if #repos == 0 then
				done({ to_settings("choose-repos", locale.s("shell.add_project.choose_repos"), locale.s("shell.add_project.no_repos_yet")) })
				return
			end
			local function owner(r: cloud.Repo): string
				return string.match(r.name, "^([^/]+)/") or r.name
			end
			-- Each owner's newest push, to order the owners by.
			local newest: { [string]: number } = {}
			for _, r in repos do
				local o = owner(r)
				newest[o] = math.max(newest[o] or 0, r.pushed or 0)
			end
			table.sort(repos, function(a, b)
				local oa, ob = owner(a), owner(b)
				if oa ~= ob then
					if newest[oa] ~= newest[ob] then
						return newest[oa] > newest[ob]
					end
					return string.lower(oa) < string.lower(ob)
				end
				if (a.pushed or 0) ~= (b.pushed or 0) then
					return (a.pushed or 0) > (b.pushed or 0)
				end
				return string.lower(a.name) < string.lower(b.name)
			end)
			-- Already a project on one of your devices: where (production's
			-- "On …"), so a Cloud one of it is a deliberate pick.
			local snap = sessions.now:peek()
			local names: { [string]: string } = {}
			for _, d in snap.devices do
				names[d.id] = d.name
			end
			local out: { Item } = {}
			for _, r in repos do
				local on: string? = nil
				for _, p in snap.projects do
					if p.github and string.lower(p.github :: string) == string.lower(r.name) and string.sub(p.device, 1, 6) ~= "cloud-" then
						on = names[p.device]
					end
				end
				local words = {}
				if on then
					table.insert(words, locale.s("shell.add_project.on_device", { device = on }))
				end
				if r.private then
					table.insert(words, locale.s("shell.add_project.private"))
				end
				local name = r.name
				table.insert(out, {
					id = `repo:{name}`,
					label = name,
					section = owner(r),
					icon = "git-branch",
					hint = if #words > 0 then table.concat(words, " · ") else nil,
					verb = locale.s("shell.add_project.add_cloud_project"),
					run = function()
						local at = cloud.device()
						if not at then
							return
						end
						-- Where a Cloud machine checks it out.
						local short = string.match(name, "([^/]+)$") or name
						M.add(at :: string, `/home/user/{short}`, true, name)
					end,
				})
			end
			done(out)
		end)
	end
	return page
end

-- Your devices, this Mac first (never Cloud's machines: they're its
-- sessions'); then Cloud, while it's on.
local function devices(): { Item }
	local out: { Item } = {}
	local here_id = sessions.here:get()
	for _, d in sessions.computers(sessions.devices:get()) do
		local here = d.here or d.id == here_id
		local item = {
			id = `device:{d.id}`,
			label = d.name,
			section = locale.s("shell.add_project.devices"),
			icon = if here then "laptop" else "remote-server",
			hint = if d.online == false and not here then locale.s("shell.add_project.offline") else nil,
			verb = locale.s("shell.add_project.open"),
			page = function(): Page
				return locations_page(d.id, d.name)
			end,
		}
		if here then
			table.insert(out, 1, item)
		else
			table.insert(out, item)
		end
	end
	if cloud.on() then
		table.insert(out, {
			id = "device:cloud",
			label = locale.s("shell.add_project.cloud"),
			section = locale.s("shell.add_project.devices"),
			icon = "cloud",
			hint = locale.s("shell.add_project.cloud_hint"),
			verb = locale.s("shell.add_project.open"),
			page = cloud_page,
		})
	end
	return out
end

local recent = ui.state("shell_add_project_recent", {} :: { string })

-- Its first step: your devices. (Its words read as they're drawn, the
-- locale shown then: its palette's root is made once.)
local function root(): Page
	return setmetatable({ id = "new-project", items = devices }, {
		__index = function(_, field: string): any
			if field == "title" then
				return locale.s("shell.add_project.title")
			elseif field == "placeholder" then
				return locale.s("shell.add_project.search_devices")
			elseif field == "empty" then
				return { title = locale.s("shell.add_project.no_devices"), hint = locale.s("shell.add_project.no_devices_hint") }
			end
			return nil
		end,
	})
end
function M.page(): Page
	M.hosted:set(true)
	return root()
end

return setmetatable(M, {
	__call = function(): Node
		return ui.div {
			id = "shell-add-project",
			CommandPalette {
				open = M.shown,
				recent = recent,
				root = root(),
			},
		}
	end,
})

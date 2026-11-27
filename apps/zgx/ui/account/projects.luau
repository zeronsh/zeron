-- Adding a project and removing one, through the engine (src/projects.rs):
-- a folder on one of your devices, browsed there (no folder dialog, this
-- Mac's included), or a repository cloned into one. Each call says which
-- device and whether it's another one (`remote`). Promises; a refusal
-- carries the engine's own words.
--
--   projects.folders(device, path?)     { path, entries = { { name, is_dir,
--                                       is_repo } }, truncated } (no path: home)
--   projects.drives(device)             { { name, path } }
--   projects.search_home(device, query) { matches = { { path, relative,
--                                       is_repo } }, indexing }
--   projects.release_home(device)
--   projects.clone(device, url, parent) { path, name }
--   projects.create(device, path, git)  the new project's id
--   projects.delete(id)                 gone, and its sessions with it
--   projects.artwork(project, done)     its repository's own picture, as the
--                                       shipping app finds it: `done` with a
--                                       file here, or nil
--   projects.upload_icon(path, crop)    the square `crop` ({ x, y, side },
--                                       pixels) of a picture, kept by the
--                                       edge: a promise of its https URL
--   projects.forget_icon(url)           one of those, let go of (replaced)
--   projects.icon_file(url)             an uploaded picture's file here (a
--                                       promise; nil when it can't be had)
--   projects.picture_size(path)         a picture's { w, h }, to crop it
--
-- A project of repositories from anywhere on its device, and the device's
-- repositories (a host saying `composed-projects-v1`; see M.composes):
--
--   projects.repositories(device, path) its repositories: { repo, truncated,
--                                       list = { { path, branch?, empty?,
--                                       location?, missing?, github? } } }
--   projects.compose(device, name, repositories, parent?)  a new folder
--                                       including them: { path }
--                                       (`repositories`: { { path, name? } })
--   projects.set_repositories(device, path, repositories)  what the project
--                                       includes now, listed as above
--   projects.local_repositories(device, refresh?)  { list = { { path, name,
--                                       branch?, empty?, github?, touched?,
--                                       kept? } }, parent, publish, truncated }
--   projects.new_repository(device, { name, parent?, empty?, publish?,
--                                       private? })  { repository, warning? }
--   projects.forget_repository(device, path)
--   projects.add_repository(device, path)  { path, name }
--   projects.composes(device)           whether its host does all that
--
-- `device` is a device's id.

local sessions = require("@account/sessions")

local M = {}

local function on(device: string): { [string]: any }
	local here = sessions.now:peek().here
	return { device = device, remote = here ~= nil and device ~= here }
end
local function with(device: string, extra: { [string]: any }): { [string]: any }
	local arg = on(device)
	for k, v in extra do
		arg[k] = v
	end
	return arg
end

function M.folders(device: string, path: string?): any
	return ui.call("projects.folders", with(device, { path = path }))
end
function M.drives(device: string): any
	return ui.call("projects.drives", on(device))
end
function M.search_home(device: string, query: string): any
	return ui.call("projects.search_home", with(device, { query = query }))
end
function M.release_home(device: string)
	ui.call("projects.release_home", on(device)):after(function() end, function() end)
end
function M.clone(device: string, url: string, parent: string): any
	return ui.call("projects.clone", with(device, { url = url, parent = parent }))
end
-- (`repo`: a Cloud project's GitHub repository, `owner/name`: it has no
-- folder to read one from.)
function M.create(device: string, path: string, git: boolean, repo: string?, name: string?): any
	return ui.call("projects.create", { device = device, path = path, git = git, repo = repo, name = name })
end
function M.repositories(device: string, path: string): any
	return ui.call("projects.repositories", with(device, { path = path }))
end
function M.compose(device: string, name: string, repositories: { any }, parent: string?): any
	return ui.call("projects.compose", with(device, { name = name, repositories = repositories, parent = parent }))
end
function M.set_repositories(device: string, path: string, repositories: { any }): any
	return ui.call("projects.set_repositories", with(device, { path = path, repositories = repositories }))
end
function M.local_repositories(device: string, refresh: boolean?): any
	return ui.call("projects.local", with(device, { refresh = refresh == true }))
end
function M.new_repository(device: string, ask: { [string]: any }): any
	return ui.call("projects.new_repository", with(device, ask))
end
function M.forget_repository(device: string, path: string): any
	return ui.call("projects.forget_repository", with(device, { path = path }))
end
function M.add_repository(device: string, path: string): any
	return ui.call("projects.add_repository", with(device, { path = path }))
end
-- Whether `device`'s host composes projects and manages its repositories
-- (it says `composed-projects-v1`); Cloud's machines never do.
function M.composes(device: string): boolean
	for _, d in sessions.devices:get() do
		if d.id == device then
			return d.composes == true and d.cloud ~= true
		end
	end
	return false
end

return M

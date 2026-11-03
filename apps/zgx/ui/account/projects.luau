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
function M.create(device: string, path: string, git: boolean, repo: string?): any
	return ui.call("projects.create", { device = device, path = path, git = git, repo = repo })
end
function M.delete(id: string): any
	return ui.call("projects.delete", id)
end
function M.artwork(p: { id: string, device: string, path: string }, done: (src: string?) -> ())
	ui.call("files.artwork", { target = { space = p.id, device = p.device, root = p.path } }):after(function(found: any)
		done(if type(found) == "table" then found.src else nil)
	end, function()
		done(nil)
	end)
end
function M.upload_icon(path: string, crop: { x: number, y: number, side: number }): any
	return ui.call("account.upload_image", { path = path, x = crop.x, y = crop.y, side = crop.side })
end
function M.forget_icon(url: string)
	ui.call("account.delete_image", { url = url }):after(function() end, function() end)
end
function M.icon_file(url: string): any
	return ui.call("account.photo_file", { url = url })
end
function M.picture_size(path: string): any
	return ui.call("account.photo_size", { path = path })
end

return M

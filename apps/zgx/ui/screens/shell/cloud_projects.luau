-- Cloud's projects, a repository once: each Cloud chat runs on a machine of
-- its own, and each machine has its own copy of the repository's project.
-- Where projects are listed for you (Settings → Projects, the new chat's
-- project on Cloud), those copies are one: the same repository (its GitHub
-- repository, else its name), reading as the newest copy someone named or
-- gave an icon, else the newest.
--
--   cloud_projects.key(p)        which repository's it is, as one row
--   cloud_projects.lead(list)    the copy a row reads as
--
-- Pure: no state, no host.

local M = {}

export type Copy = { id: string, name: string, path: string, github: string?, created: number?, icon: any? }

function M.key(p: Copy): string
	local repo = p.github
	return if repo and repo ~= "" then `cloud:repo:{string.lower(repo)}` else `cloud:name:{p.name}`
end

-- Named or given an icon by someone (not just as it was made: its
-- repository's name, no icon).
local function customised(p: Copy): boolean
	local made = string.match(p.github or p.path or "", "([^/\\]+)$")
	return p.icon ~= nil or (made ~= nil and p.name ~= made)
end

local function newer(a: Copy, b: Copy): boolean
	local x, y = a.created or 0, b.created or 0
	if x ~= y then
		return x > y
	end
	return a.id > b.id
end

function M.lead<T>(list: { T & Copy }): T & Copy
	local best: (T & Copy)? = nil
	local best_custom = false
	for _, p in list do
		local custom = customised(p)
		if best == nil or (custom and not best_custom) or (custom == best_custom and newer(p, best :: any)) then
			best, best_custom = p, custom
		end
	end
	return best :: any
end

return M

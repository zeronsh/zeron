-- Cloud, as the host reads it from the engine (src/cloud.rs): whether the
-- account has Cloud and it's on, the credits left, and the GitHub
-- repositories Cloud can open. Settings → Cloud shows and changes it; the
-- new chat's Work in offers Cloud for a project whose repository it can
-- open (screens/chat/where).
--
--   cloud.now:get()   { phase = "unknown" | "loading" | "ready" | "failed",
--                       status?, repos?, repos_loading, failure? }
--   cloud.load()        Cloud's status, then its repositories while it's on
--   cloud.load_repos()  the repositories again (after choosing on GitHub);
--                       `repos_failure` says why one didn't come
--                       ("github_not_connected", "offline", "failed")
--   cloud.offered()     signed in and the account has Cloud (tracked)
--   cloud.on()          …and it's on (tracked)
--   cloud.reaches(name) the repository of that `owner/name`, if Cloud can
--                       open it (tracked)
--   cloud.usage(from, to)              a promise of the credits used per day
--   cloud.branches(repo, default?)     a promise of a repository's branches
--
-- A status: { state, available, device?, credits?, error?, failed_action?,
-- awake?, max_awake? }. A repository: { name, private, branch?, pushed }.
-- Every function here is a field, so a fixture answers in the host's place.

local auth = require("@account/auth")

local M = {}

export type Status = {
	state: string,
	available: boolean,
	device: string?,
	credits: number?,
	error: string?,
	failed_action: string?,
	awake: number?,
	max_awake: number?,
}
export type Repo = { name: string, private: boolean, branch: string?, clone_url: string?, pushed: number }
export type Now = {
	phase: string,
	status: Status?,
	repos: { Repo }?,
	repos_loading: boolean,
	repos_failure: string?,
	failure: string?,
}

M.now = ui.state("account_cloud", { phase = "unknown", repos_loading = false } :: Now)
-- A read still out when the scripts reload never answers these ones (its
-- promise was the old scripts'): asked again from here.
do
	local was = M.now:peek()
	if was.phase == "loading" or was.repos_loading then
		local next = table.clone(was) :: any
		next.phase = if was.status then "ready" else "unknown"
		next.repos_loading = false
		M.now:set(next)
	end
end

local function patch(fields: { [string]: any })
	local next = table.clone(M.now:peek()) :: any
	for k, v in fields do
		next[k] = v
	end
	M.now:set(next)
end

-- Cloud is on: a Cloud of the account's to run on (not being turned off).
function M.is_on(s: Status?): boolean
	return s ~= nil and s.available and s.device ~= nil and s.state ~= "off" and s.state ~= "deleting"
end

function M.offered(): boolean
	local s = M.now:get().status
	return auth.state:get() == "signed_in" and s ~= nil and s.available
end

function M.on(): boolean
	return M.offered() and M.is_on(M.now:get().status)
end

function M.reaches(name: string?): Repo?
	if not name or not M.on() then
		return nil
	end
	local wanted = string.lower(name)
	for _, r in M.now:get().repos or {} do
		if string.lower(r.name) == wanted then
			return r
		end
	end
	return nil
end

-- Take a status (a read, or an action's answer); its repositories follow
-- while Cloud is on.
function M.take(s: Status)
	local was_on = M.is_on(M.now:peek().status)
	patch { phase = "ready", status = s, failure = nil :: any }
	if M.is_on(s) then
		if not was_on or M.now:peek().repos == nil then
			M.load_repos()
		end
	elseif M.now:peek().repos ~= nil then
		patch { repos = nil :: any }
	end
end

function M.load()
	if M.now:peek().phase == "unknown" or M.now:peek().phase == "failed" then
		patch { phase = "loading" }
	end
	ui.call("cloud.status", nil):after(function(s: any)
		M.take(s)
	end, function(e: any)
		-- (What was known stays: a failed re-read says so beside it.)
		if M.now:peek().status then
			patch { failure = e.message }
		else
			patch { phase = "failed", failure = e.message }
		end
	end)
end

-- (`done`: called once the read lands, either way.)
function M.load_repos(done: (() -> ())?)
	patch { repos_loading = true }
	ui.call("cloud.repos", nil):after(function(list: any)
		patch { repos = list or {}, repos_loading = false, repos_failure = nil :: any }
		if done then
			done()
		end
	end, function(e: any)
		-- (A failed read keeps what was known, but GitHub gone is gone.)
		local word = if type(e) == "table" and type(e.message) == "string" then e.message else "failed"
		if word == "github_not_connected" then
			patch { repos = {}, repos_loading = false, repos_failure = word }
		else
			patch { repos_loading = false, repos_failure = word }
		end
		if done then
			done()
		end
	end)
end

function M.usage(from: string, to: string): any
	return ui.call("cloud.usage", { from = from, to = to })
end

function M.branches(repo: string, default: string?): any
	return ui.call("cloud.branches", { repo = repo, default = default })
end

-- The logical Cloud device's id, while Cloud is on (where a Cloud project
-- lives).
function M.device(): string?
	local s = M.now:get().status
	return if M.on() and s then s.device else nil
end

-- Signed out, nothing of the account's Cloud stays for the next one.
function M.forget()
	M.now:set { phase = "unknown", repos_loading = false }
end

return M

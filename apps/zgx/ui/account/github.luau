-- GitHub for Cloud agents, as the host keeps it (src/github.rs): whether
-- the account's GitHub is connected for Cloud, and GitHub's device flow to
-- connect it (a code to enter on GitHub, approved there).
--
--   github.now:get()   { phase = "checking" | "unavailable" | "idle"
--                        | "starting" | "waiting" | "failed" | "connected",
--                        code?, account?, accounts?, already?,
--                        repos = { "owner/name" }, failure? }
--   github.accounts(now)  the logins connected for Cloud, whatever host answered
--   github.many(now)      whether there's more than one
--   github.connecting(now) whether a device flow is under way
--   github.load()        where the account stands (and the first time)
--   github.connect()     asks GitHub for a code, then waits for the approval
--   github.cancel()      stops waiting
--   github.open()        GitHub's page for the code
--   github.choose()      GitHub's page for the repositories Cloud can open
--   github.disconnect()   all of GitHub, as an engine with one login knows it
--   github.forget(account) one login; the others carry on
--
-- `failure` is said for the screen already: what went wrong and what to do.
-- `already`: the login just approved was connected before, so nothing was added.

local M = {}

export type Account = {
	-- (None for the one login an older engine or vault names: `forget` is
	-- then all of GitHub.)
	slot: string?,
	login: string,
	avatar: string?,
	-- "connected" | "needs_reconnect"
	status: string,
}
export type Now = {
	phase: string,
	code: string?,
	account: string?,
	accounts: { Account }?,
	already: string?,
	repos: { string },
	failure: string?,
}

M.now = ui.shared("github", { phase = "checking", repos = {} } :: Now)

-- (Not while a code is out: a load starts the host's flow over, which ends
-- the wait for the approval and takes the code off the screen. A screen
-- building again, as each hot reload builds it, asks for a load.)
function M.load()
	local p = M.now:peek().phase
	if p == "starting" or p == "waiting" then
		return
	end
	ui.invoke("github.load")
end

-- The logins connected for Cloud. An engine or vault from before several
-- logins lists none, and names the one it has.
function M.accounts(now: Now): { Account }
	local list = now.accounts
	if list and #list > 0 then
		return list
	end
	if now.phase == "connected" and now.account then
		return { { login = now.account, status = "connected" } }
	end
	return {}
end

function M.many(now: Now): boolean
	return #M.accounts(now) > 1
end

function M.connecting(now: Now): boolean
	return now.phase == "starting" or now.phase == "waiting"
end

function M.connect()
	ui.invoke("github.connect")
end

function M.cancel()
	ui.invoke("github.cancel")
end

function M.open()
	ui.invoke("github.open")
end

function M.choose()
	ui.invoke("github.choose")
end

function M.disconnect()
	ui.invoke("github.disconnect")
end

function M.forget(account: Account)
	if account.slot then
		ui.invoke("github.forget", { slot = account.slot })
	else
		M.disconnect()
	end
end

return M

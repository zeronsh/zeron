-- GitHub for Cloud agents, as the host keeps it (src/github.rs): whether
-- the account's GitHub is connected for Cloud, and GitHub's device flow to
-- connect it (a code to enter on GitHub, approved there).
--
--   github.now:get()   { phase = "checking" | "unavailable" | "idle"
--                        | "starting" | "waiting" | "failed" | "connected",
--                        code?, account?, repos = { "owner/name" }, failure? }
--   github.load()        where the account stands (and the first time)
--   github.connect()     asks GitHub for a code, then waits for the approval
--   github.cancel()      stops waiting
--   github.open()        GitHub's page for the code
--   github.choose()      GitHub's page for the repositories Cloud can open
--   github.disconnect()
--
-- `failure` is said for the screen already: what went wrong and what to do.

local M = {}

export type Now = {
	phase: string,
	code: string?,
	account: string?,
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

return M

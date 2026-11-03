-- The coding agents on this Mac, as the host keeps them (src/agents.rs):
-- what's installed, each agent's accounts, and signing into one. Scripts
-- read `agents.now` and ask for what changes it. Each agent carries all it
-- shows (its accounts, its sign-in's `login`), so a page keys its rows by
-- `id` and they update in place (`ui.each`'s row state), never rebuilt.
--
--   agents.now:get()     { state = "looking" | "ready" | "unavailable",
--                          list = { Agent } }
--   agents.load()        looks again (and the first time)
--   agents.sign_in(id)   a new account for that agent, through the browser
--   agents.cancel(id)    gives up that sign-in
--   agents.dismiss(id)   forgets one that failed
--   agents.set_enabled(id, on)
--   agents.switch(id, account_id)
--   agents.remove(id, account_id)
--   agents.install(id)   (minutes; `installing` on the agent meanwhile)
--   agents.target(device) whose agents: another of your devices (its id;
--                        the engine forwards every call there), or nil
--                        for this Mac's. `now.target` says which.
--
-- An agent: { id, name, mark?, installed, can_install, enabled, version?,
-- installing, login, accounts }. `mark` is ds.Mark's name for it (nil:
-- there is none); `login` its sign-in, "idle", "waiting", "code" (a code
-- to paste back: `complete`) or "failed". An
-- account: { id, email, plan, session, week, active, session_resets?,
-- week_resets? }, `session` and `week` its plan's two windows, 0 to 1, and
-- when each starts over (seconds since the epoch).
--
-- A Codex account also has what it banks: `banked` (how many resets) and
-- `resets`, soonest to expire first ({ id, title, scope?, expires? },
-- `expires` in seconds since the epoch), and its last reset's word,
-- `reset = { state, why? }`: "using", "reset" (the limits started over:
-- for five minutes), "nothing" (no window needed it: still banked), "gone"
-- (no longer available) or "failed" (`why`: mismatch, signed_out, missing,
-- offline or failed), the last three until dismissed.
--   agents.use_reset(account, credit)  spends one, on the device shown
--   agents.dismiss_reset(account)      forgets its word

local M = {}

export type Reset = { id: string, title: string, scope: string?, expires: number? }
export type ResetWord = { state: string, why: string? }
export type Account = {
	id: string,
	email: string,
	plan: string,
	session: number,
	week: number,
	active: boolean,
	session_resets: number?,
	week_resets: number?,
	banked: number?,
	resets: { Reset }?,
	reset: ResetWord?,
}
export type Agent = {
	id: string,
	name: string,
	mark: string?,
	installed: boolean,
	can_install: boolean,
	enabled: boolean,
	version: string?,
	installing: boolean,
	login: string,
	accounts: { Account },
}
export type Now = { state: string, list: { Agent }, target: string? }

M.now = ui.shared("agents", { state = "looking", list = {} } :: Now)

local function quietly(): (any) -> ()
	return function() end
end

function M.target(device: string?)
	ui.invoke("agents.target", device or "")
end

function M.load()
	ui.invoke("agents.load")
end

function M.sign_in(id: string)
	ui.call("agents.sign_in", id):after(function() end, quietly())
end

function M.cancel(id: string)
	ui.invoke("agents.cancel", id)
end

-- An agent from the ACP registry signed in by a way it offers (one of its
-- `login_methods`).
function M.sign_in_with(id: string, method: string)
	ui.call("agents.sign_in", { id = id, method = method }):after(function() end, quietly())
end

-- A key such an agent reads from its environment (`var`), kept there.
function M.set_key(id: string, var: string, value: string)
	ui.call("agents.set_key", { id = id, var = var, value = value }):after(function() end, quietly())
end

-- An agent from the ACP registry signed out (its own `logout`).
function M.sign_out(id: string)
	ui.call("agents.sign_out", id):after(function() end, quietly())
end

-- The page a sign-in under way opened (`login_page`), opened again.
function M.open_page(id: string)
	ui.invoke("agents.open_page", id)
end

-- A sign-in waiting for a code ("code"), finished with the one pasted back
-- (a wrong one: its `login` "failed").
function M.complete(id: string, code: string)
	ui.call("agents.complete", { id = id, code = code }):after(function() end, quietly())
end

function M.dismiss(id: string)
	ui.invoke("agents.dismiss", id)
end

function M.set_enabled(id: string, on: boolean)
	ui.call("agents.set_enabled", { id = id, enabled = on }):after(function() end, quietly())
end

function M.switch(id: string, account: string)
	ui.call("agents.switch", { id = id, account = account }):after(function() end, quietly())
end

function M.remove(id: string, account: string)
	ui.call("agents.remove", { id = id, account = account }):after(function() end, quietly())
end

function M.install(id: string)
	ui.call("agents.install", id):after(function() end, quietly())
end

-- (How it went lands on the account's row: `reset`.)
function M.use_reset(account: string, credit: string)
	ui.call("agents.use_reset", { account = account, credit = credit }):after(function() end, quietly())
end

function M.dismiss_reset(account: string)
	ui.invoke("agents.dismiss_reset", account)
end

return M

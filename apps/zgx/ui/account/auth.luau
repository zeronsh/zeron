-- Your account, as the host keeps it (src/auth.rs): who is signed in, and
-- the way in, through the browser. The browser's sign-in makes an account
-- or signs into one, so "Create an account" and "I already have an account"
-- start the same thing, each opening on its own screen. The session lives
-- in the host and the Keychain; a script never holds a token, it asks for
-- what needs one
-- (account/api).
--
--   auth.state:get()        "restoring" (a launch, until the engine answers:
--                           with a session kept, for as long as that takes),
--                           "signed_out", "waiting" (on the browser) or
--                           "signed_in"
--   auth.user:get()         { id, email, first_name? }, signed in
--   auth.failure:get()      why the last sign-in didn't finish (below)
--   auth.sign_in("create" | "existing", function(user, failure) … end)
--   auth.cancel()           gives up waiting on the browser
--   auth.sign_out(function() … end)
--   auth.says(failure)      that failure as a line the screen can show
--
-- What a sign-in can end in besides a user: "cancelled" (asked for here),
-- "denied" (turned down in the browser), "timed_out" (the browser never
-- came back: closed, or left), "offline", "failed"; and, signed out without
-- asking, "expired".

local M = {}

export type State = "restoring" | "signed_out" | "waiting" | "signed_in"
export type Failure = "cancelled" | "denied" | "timed_out" | "offline" | "expired" | "failed"
export type User = { id: string, email: string, first_name: string? }
-- `organization_id`: the organization the session works in (Settings').
export type Account = { state: State, user: User?, failure: Failure?, env: string, organization_id: string? }

-- The host's, kept current as the account changes. (A window it gives no
-- account, the intro's, reads signed out.)
M.account = ui.shared("account", { state = "signed_out", env = "none" } :: Account)

M.state = ui.computed(function(): State
	return (M.account:get() :: Account).state
end)
M.user = ui.computed(function(): User?
	return (M.account:get() :: Account).user
end)
M.failure = ui.computed(function(): Failure?
	return (M.account:get() :: Account).failure
end)
M.signed_in = ui.computed(function(): boolean
	return M.state:get() == "signed_in"
end)
-- Which Zeron it signs into: "staging", "production" or "custom".
M.env = ui.computed(function(): string
	return (M.account:get() :: Account).env
end)

-- Each sign-in started; only the last one's answer is passed on (a second
-- click gives up the first, whose "cancelled" nobody needs to hear).
local attempt = 0

-- Opens the browser on the sign-in and waits for it. `kind`: "create"
-- opens on making an account and asks afresh (a browser still signed into
-- someone doesn't hand that account back unasked); "existing" opens on
-- signing in. `done(user)` once signed in, or `done(nil, failure)`;
-- already signed in, at once.
export type Kind = "create" | "existing"
function M.sign_in(kind: Kind, done: ((User?, Failure?) -> ())?)
	attempt += 1
	local mine = attempt
	ui.call("account.sign_in", kind):after(function(answer: unknown)
		-- (The host's answer: the account, signed in.)
		local account = answer :: Account
		if mine == attempt and done then
			done(account.user, nil)
		end
	end, function(err: any)
		if mine == attempt and done then
			done(nil, err.message :: Failure)
		end
	end)
end

-- Gives up waiting on the browser (its sign-in ends "cancelled").
function M.cancel()
	attempt += 1
	ui.invoke("account.cancel")
end

function M.sign_out(done: (() -> ())?)
	attempt += 1
	ui.call("account.sign_out"):after(function()
		if done then
			done()
		end
	end, function() end)
end

-- What each way a sign-in can end says, said as what happened and what to
-- do, never how (its message's key; said in the locale shown, read where
-- it's drawn).
local SAYS: { [string]: string } = {
	cancelled = "account.auth.says.cancelled",
	denied = "account.auth.says.denied",
	timed_out = "account.auth.says.timed_out",
	offline = "account.auth.says.offline",
	expired = "account.auth.says.expired",
	failed = "account.auth.says.failed",
}

function M.says(failure: string?): string?
	return if failure then ui.t(SAYS[failure] or SAYS.failed):get() else nil
end

return M

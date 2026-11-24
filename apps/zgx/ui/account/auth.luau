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
--
-- The server the account lives on: Zeron's, or one someone runs themselves
-- (docs/self-hosting.md), its sign-in its own.
--
--   auth.server:get()       { kind, name?, host, url, zeron_host, guide }
--   auth.server_label(s)    what it's called: "Zeron", or its own name
--   auth.find_server(address, function(found, failure) … end)
--   auth.use_server(found | nil, function(failure?) … end)   nil: Zeron's
--   auth.server_says(failure)  why one couldn't be used, as a line

local M = {}

export type State = "restoring" | "signed_out" | "waiting" | "signed_in"
export type Failure = "cancelled" | "denied" | "timed_out" | "offline" | "expired" | "failed"
export type User = { id: string, email: string, first_name: string? }
-- Where the account lives. `kind`: "zeron" (the build's own), "self_hosted"
-- (one chosen) or "environment" (a development build's ZERON_ZGX_EDGE named
-- it at launch, and only a launch changes it). `name`, a self-hosted one's own; `host`, its
-- address as shown; `zeron_host`, Zeron's own, as it's offered beside it;
-- `guide`, how to run one.
export type ServerKind = "zeron" | "self_hosted" | "environment"
export type Server = { kind: ServerKind, name: string?, host: string, url: string, zeron_host: string, guide: string }
-- `organization_id`: the organization the session works in (Settings').
export type Account = { state: State, user: User?, failure: Failure?, env: string, server: Server?, organization_id: string? }

-- The host's, kept current as the account changes. (The intro's window
-- reads signed out, on the server setup's will sign in on.)
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

-- -- the server ---------------------------------------------------------

-- Zeron's own, as a window the host gives no account (a fixture's) has it.
local ZERON: Server = {
	kind = "zeron",
	host = "edge.zeron.sh",
	url = "https://edge.zeron.sh",
	zeron_host = "edge.zeron.sh",
	guide = "https://github.com/zeronsh/zeron/blob/main/docs/self-hosting.md",
}

M.server = ui.computed(function(): Server
	return (M.account:get() :: Account).server or ZERON
end)

-- What a server is called: Zeron's is "Zeron" (whichever of its own the
-- build signs into); another, the name its owner gave it, else its address.
function M.server_label(server: Server?): string
	local s = server or ZERON
	if s.kind == "zeron" then
		return ui.t("account.server.zeron"):get()
	end
	return if s.name and s.name ~= "" then s.name else s.host
end

-- A server found at an address, as `account.find_server` answers: `zeron`,
-- it's Zeron's own; `current`, it's where the account lives now. Handed
-- back as it is to `use_server`.
export type Found = { zeron: boolean, current: boolean, host: string, url: string, workos_client_id: string, name: string? }
-- Why there's none to use: `error`, a word (below); `host`, the address
-- asked about; `message`, the host's own words for a word this doesn't
-- know; `missing`, what a server not set up lacks; `status`, what one not
-- answering said.
export type NotFound = { error: string, host: string, message: string, missing: string?, status: number? }

-- Each lookup asked for; only the last one's answer is passed on (an
-- address changed while one was on its way gives it up).
local looking = 0

-- Looks for a Zeron server at `address`, as typed ("zeron.acme.com").
-- `done(found)`, or `done(nil, why)`.
function M.find_server(address: string, done: (Found?, NotFound?) -> ())
	looking += 1
	local mine = looking
	ui.call("account.find_server", address):after(function(answer: unknown)
		if mine ~= looking then
			return
		end
		local a = answer :: any
		if a and a.error then
			done(nil, a :: NotFound)
		else
			done(a :: Found, nil)
		end
	end, function(err: any)
		if mine == looking then
			done(nil, { error = err.message or "failed", host = address, message = "" })
		end
	end)
end

-- Gives up the lookup on its way: its answer is heard by no one.
function M.forget_lookup()
	looking += 1
end

-- Signs in on `found` from now on (nil: Zeron's). Signed in, this signs out
-- first: an account lives on one server. `done(nil)` once it's the one, or
-- `done(failure)`: "offline" (the engine can't be reached to sign out),
-- "set_by_environment", "failed".
function M.use_server(found: Found?, done: ((string?) -> ())?)
	attempt += 1
	ui.call("account.use_server", found):after(function()
		if done then
			done(nil)
		end
	end, function(err: any)
		if done then
			done(if type(err) == "table" and err.message then err.message else "failed")
		end
	end)
end

-- Why a server couldn't be used, said in the locale shown: an address it
-- couldn't take, a server it couldn't reach or sign in to, or the switch
-- itself refused. A word this doesn't know says the host's words.
local SERVER_SAYS: { [string]: string } = {
	empty = "account.server.says.empty",
	invalid = "account.server.says.invalid",
	scheme = "account.server.says.scheme",
	credentials = "account.server.says.credentials",
	path = "account.server.says.path",
	insecure = "account.server.says.insecure",
	unreachable = "account.server.says.unreachable",
	not_zeron = "account.server.says.not_zeron",
	outdated = "account.server.says.outdated",
	needs_newer_app = "account.server.says.needs_newer_app",
	dev_server = "account.server.says.dev_server",
	not_configured = "account.server.says.not_configured",
	unavailable = "account.server.says.unavailable",
	offline = "account.server.says.offline",
	set_by_environment = "account.server.says.set_by_environment",
	failed = "account.server.says.failed",
}

function M.server_says(why: NotFound): string
	if why.error == "not_configured" and why.missing and why.missing ~= "" then
		return ui.t("account.server.says.not_configured_missing", { host = why.host, missing = why.missing }):get()
	end
	local key = SERVER_SAYS[why.error]
	if key then
		return ui.t(key, { host = why.host, status = tostring(why.status or "") }):get()
	end
	return if why.message ~= "" then why.message else ui.t(SERVER_SAYS.failed):get()
end

return M

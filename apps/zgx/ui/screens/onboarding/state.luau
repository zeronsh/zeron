-- Onboarding's state. `step` is plain state, kept across hot reloads, so a
-- save shows the step being worked on; `ZERON_ZGX_STEP` (through the host's
-- `onboarding.start`) picks where a launch starts.

local api = require("@account/api")

export type Step = "signin" | "appearance" | "username" | "card" | "org" | "invite" | "github" | "providers" | "done"
export type Path = "signed_in" | "skipped"
export type Appearance = "system" | "dark" | "light"
-- What sign-in was started for: a new account, or one already had.
export type Account = "create" | "existing"
-- Where sign-in stands: not started, waiting on the browser, or failed.
export type SignIn = "ready" | "waiting" | "failed"

local M = {}

-- The two ways through, in order. Signed in: the look, then the account's
-- name and card, naming its organization and inviting to it, GitHub, the
-- agents. Without an account: the look and the agents. Both open on sign-in
-- and end on done.
M.PATHS = {
	signed_in = { "signin", "appearance", "username", "card", "org", "invite", "github", "providers", "done" },
	skipped = { "signin", "appearance", "providers", "done" },
} :: { [Path]: { Step } }

-- Which steps are pages that are finished once and remembered, and where:
-- an account's on the account (Zeron's `/onboarding`, so another Mac
-- passes over them), a device's on this Mac. Sign-in and done are not
-- pages. A page added later (a "what's new") gets an id of its own, and
-- anyone who finished the others sees only it; a page redesigned to be
-- seen again gets a new id.
export type Scope = "account" | "device"
M.PAGES = {
	appearance = "device",
	username = "account",
	card = "account",
	org = "account",
	invite = "account",
	github = "account",
	providers = "device",
} :: { [string]: Scope }
-- Pages that announce something (a release's what's new): finishing setup
-- marks them all done, so a new account never meets news of what it's
-- seeing for the first time.
M.NEWS = {} :: { string }

-- The device's pages finished on this Mac.
M.device_done = ui.persist("onboarding_done", {} :: { [string]: boolean })
-- The pages finished before this run (the account's, as Zeron answered at
-- sign-in, and this Mac's): the run opens on the first one left. They stay
-- in the path, so Back and Continue go through every step in order from
-- there, and the dots don't move. Taken once, as the run starts.
M.passed = ui.state("passed", {} :: { [string]: boolean })
-- `ZERON_ZGX_REPLAY`: every page, as if none were finished.
M.replay = ui.shared("onboarding.replay", false):peek() == true

local start = ui.shared("onboarding.start", "signin"):peek() :: Step
M.step = ui.state("step", start)
-- Which way through: signed in (a sign-in, or a session that was kept), or
-- skipped at sign-in.
M.path = ui.state("path", "skipped" :: Path)
-- The account had its username when it signed in: claiming one isn't a
-- step of this run (`username` leaves the path; its card still shows).
M.name_done = ui.state("name_done", false)
-- A username was claimed in this run.
M.claimed = ui.state("claimed", false)
-- The account's card, once claimed or asked for (kept as state, so a hot
-- reload on Your card neither asks Zeron again nor deals it again), and
-- whether it has been dealt (its arrival plays once).
M.card = ui.state("card", nil :: api.Card?)
M.card_dealt = ui.state("card_dealt", false)
-- Back at Sign in after signing out: its wall rises again (once).
M.wall_again = ui.state("wall_again", false)

-- Each time a step opens, a visit (screens/visits, as Settings' pages have
-- them): what a step asks Zeron as it opens (the account's GitHub, its
-- agents, its username) is asked once a visit, not again as a hot reload
-- builds the step again. An answer a step waits for (its username, its
-- organization) is asked until answered in this visit.
local visits = require("@app/screens/visits").new("onboarding", function(): string?
	return M.step:get()
end)
M.visit = visits.visit
M.once_a_visit = visits.once
M.ask_until_answered = visits.ask_until_answered
M.answered = visits.answered
M.answer = visits.answer
-- The account was in an organization when it signed in: it works in that
-- one, and naming one isn't a step of this run (nor inviting, unless it's
-- the account's own: `org_admin`).
M.org_done = ui.state("org_done", false)
-- The organization worked in is the account's own (made in this run, or
-- its admin): it can invite to it.
M.org_admin = ui.state("org_admin", false)
-- The organization worked in, by name (You're all set says it).
M.org_name = ui.state("org_name", "")
-- Who was invited in this run: Invite lists them again on a return, and
-- You're all set says so.
M.invited = ui.state("invited", {} :: { string })
M.account = ui.state("account", "create" :: Account)
M.signin = ui.state("signin", "ready" :: SignIn)
M.appearance = ui.persist("appearance", "system" :: Appearance)
-- The organization worked in: the one made, or the one the account was in.
M.last_org = ui.persist("last_org", "")

-- The active path's steps, less the ones this run passes over.
function M.order(): { Step }
	local order = M.PATHS[M.path:get()]
	local name_done, org_done = M.name_done:get(), M.org_done:get()
	local out: { Step } = {}
	for _, s in order do
		local passed = (name_done and s == "username")
			or (org_done and (s == "org" or (s == "invite" and not M.org_admin:get())))
		if not passed then
			table.insert(out, s)
		end
	end
	return out
end

-- Where a step stands among the numbered ones (all but sign-in; done is
-- the last), and how many there are: the dots. Every step of the way through counts,
-- the ones passed over in this run too (a username already had, an
-- organization already in, pages finished before), so the dots stay put
-- as the run learns about the account. Nil for an unnumbered step.
function M.position(step: Step): (number?, number)
	local n: number? = nil
	local count = 0
	for _, s in M.PATHS[M.path:get()] do
		if s ~= "signin" then
			count += 1
			if s == step then
				n = count
			end
		end
	end
	return n, count
end

return M

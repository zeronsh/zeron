-- What setup asks of Zeron once you're signed in: your username, whether
-- one is free, claiming it, and a member's card. Each is a host call
-- (src/auth.rs) that carries the account's token, so it returns a promise:
-- `:after(on_value, on_error)`, or its `done`, `value` and `error` read by
-- an interface. An error's `message` is a word for what happened (below),
-- and `api.says` turns it into a line.
--
--   api.username()          your username, or nil
--   api.check(name)         { available, reason? = "invalid" | "reserved" | "taken" }
--   api.claim(name)         your card, the name now yours for good
--   api.card(name)          that member's card, or nil
--   api.orgs()              the organizations you're in: { id, organization_id, name }
--   api.create_org(name)    yours, you its first member: { organization_id };
--                           one already made answers with it (`existing`)
--   api.choose_org(id)      work in it from now on (the session scoped to it;
--                           refused or failed, the session stays as it was)
--
-- A card: { username, first_name, number, joined_on ("2026-06-30"),
-- badges = { { id, name, icon? } }, scene (from 0), holo, sky }. The face
-- fields draw it (`api.member(card)` for ds.MemberCard); the member's id
-- never leaves the server.
--
-- What can go wrong: "offline", "signed_out", "taken", "reserved",
-- "invalid", "already_claimed" (one each, for good), "not_ready" (a new
-- account a moment from ready), "rate_limited", "failed".

local auth = require("@account/auth")

local M = {}

export type Check = { available: boolean, reason: string? }
export type Badge = { id: string, name: string, icon: string? }
export type Card = {
	username: string,
	first_name: string,
	number: number,
	joined_on: string,
	badges: { Badge },
	scene: number,
	holo: string,
	sky: number,
}

function M.username(): any
	return ui.call("account.username")
end

function M.check(name: string): any
	return ui.call("account.check_username", name)
end

function M.claim(name: string): any
	return ui.call("account.claim_username", name)
end

function M.card(name: string): any
	return ui.call("account.card", name)
end

-- `role` is "admin" for the account's own organization.
export type Org = { id: string, organization_id: string, name: string, role: string? }

function M.orgs(): any
	return ui.call("account.orgs")
end

function M.create_org(name: string): any
	return ui.call("account.create_org", name)
end

function M.choose_org(organization_id: string): any
	return ui.call("account.choose_org", organization_id)
end

-- The onboarding pages the account has finished, by id.
function M.onboarding(): any
	return ui.call("account.onboarding")
end

-- Records onboarding pages as finished for the account.
function M.onboarded(ids: { string }): any
	return ui.call("account.onboarded", ids)
end

-- What inviting answered: who was sent one, and who wasn't and why
-- (invalid, exists or failed).
export type Invited = { sent: { string }, failed: { { email: string, error: string } } }

-- Invites `emails` to the organization as members; Zeron sends the emails.
function M.invite(organization_id: string, emails: { string }): any
	return ui.call("account.invite", { organization_id = organization_id, emails = emails })
end

-- The username rules, the server's (edge/src/username.ts): 3 to 20 letters,
-- digits, _ or -, starting with a letter or digit, kept lowercase. Reserved
-- names are the server's to say (`check` answers "reserved").
M.MIN, M.MAX = 3, 20

-- What was typed, as a username is kept: lowercase, no leading @, no
-- spaces round it.
function M.normalise(raw: string): string
	local s = string.lower(raw)
	s = string.gsub(s, "^%s+", "")
	s = string.gsub(s, "%s+$", "")
	s = string.gsub(s, "^@", "")
	return s
end

-- What stands in the way before anyone is asked: "empty", "invalid",
-- "start", "short", "long"; or nothing.
function M.problem(u: string): string?
	if u == "" then
		return "empty"
	elseif string.find(u, "[^%w_%-]") then
		return "invalid"
	elseif not string.find(u, "^%w") then
		return "start"
	elseif #u < M.MIN then
		return "short"
	elseif #u > M.MAX then
		return "long"
	end
	return nil
end

-- Where a card lives, as shown (`zeron.sh/c/mara`) and as shared (the
-- whole link; staging's own while setup signs into staging).
function M.address(name: string): string
	return `zeron.sh/c/{name}`
end

function M.link(name: string): string
	local host = if auth.env:peek() == "staging" then "staging.zeron.sh" else "zeron.sh"
	return `https://{host}/c/{name}`
end

-- "2026-06-30" as a card prints it: "Jun 30, 2026" (in the locale shown,
-- ds/locale's day: read where it's drawn).
function M.since(joined_on: string): string
	local y, m, d = string.match(joined_on, "^(%d+)-(%d+)-(%d+)$")
	local month = tonumber(m or "")
	if not y or not month or month < 1 or month > 12 then
		return joined_on
	end
	-- (Strings, so the year isn't grouped: "2,026".)
	return ui.t("time.date", { m = tostring(month), day = tostring(tonumber(d)), year = y }):get()
end

-- How served badges look, by id: maintainer is a dark (ink) pill,
-- anything else an outline one. card-web's BADGE_STYLES (site/card.js) keeps
-- the same table, so a card reads alike in the app and on the web.
local BADGE_STYLES: { [string]: string } = { maintainer = "ink" }

-- A card's face as ds.MemberCard takes it: the scene, foil and sky the
-- server gave, the member's number, first name, day and badges.
function M.member(card: Card): { [string]: any }
	local badges = {}
	for _, b in card.badges do
		table.insert(badges, { label = b.name, style = BADGE_STYLES[b.id] or "outline" })
	end
	return {
		number = card.number,
		name = card.first_name,
		since = M.since(card.joined_on),
		scene = card.scene,
		holo = card.holo,
		sky = card.sky,
		badges = if #badges > 0 then badges else nil,
	}
end

-- What each failure says, as what happened (its message's key; said in
-- the locale shown, read where it's drawn).
local SAYS: { [string]: string } = {
	offline = "account.api.says.offline",
	signed_out = "account.api.says.signed_out",
	taken = "account.api.says.taken",
	reserved = "account.api.says.reserved",
	invalid = "account.api.says.invalid",
	already_claimed = "account.api.says.already_claimed",
	not_ready = "account.api.says.not_ready",
	rate_limited = "account.api.says.rate_limited",
	refused = "account.api.says.refused",
	failed = "account.api.says.failed",
}

function M.says(word: string?): string
	return ui.t(SAYS[word or "failed"] or SAYS.failed):get()
end

return M

-- The account's side of Settings: your profile and card, the organization
-- you work in, its members and its open invitations. Each is the edge's,
-- through the host (src/settings.rs, src/auth.rs); the pages read these
-- states and ask for what changes them. A list's rows carry an `id` (a
-- membership's, an invitation's), so a page keys its rows by it and they
-- change in place. Every change shows at once and goes back if the edge
-- refuses it, saying why in a toast.
--
--   store.me:get()            { first_name?, last_name?, name?, email? }?
--   store.card:get()          your card (account/api's Card), or nil
--   store.username:get()      your username, or nil
--   store.org:get()           { organization_id, name, role? }, the one worked in
--   store.members:get()       { Member }
--   store.invitations:get()   { Invitation } (an admin's only)
--   store.status:get()[what]  "loading" | "ready" | "failed" (me, card, org,
--                             members, invitations)
--   store.load_* / rename / rename_org / remove / set_role / revoke /
--   resend / invite

local host = require("@app/screens/settings/host")
local api = require("@account/api")
local auth = require("@account/auth")
local toast = require("@ds/toast")
local PrivateText = require("@ds/private_text")

local M = {}

export type Me = {
	first_name: string?,
	last_name: string?,
	name: string?,
	email: string?,
	-- What you do, under your name; your photo's address (your own, else
	-- the one your sign-in gave).
	title: string?,
	avatar: string?,
}
export type Member = {
	id: string,
	name: string?,
	email: string?,
	role: string?,
	you: boolean?,
	avatar: string?,
	title: string?,
	username: string?,
	joined_at: string?,
}
export type Invitation = { id: string, email: string, state: string?, expires_at: string?, created_at: string? }
export type Org = { organization_id: string, name: string, role: string? }

M.me = ui.state("settings_me", nil :: Me?)
M.card = ui.state("settings_card", nil :: api.Card?)
M.username = ui.state("settings_username", nil :: string?)
M.org = ui.state("settings_org", nil :: Org?)
M.members = ui.state("settings_members", {} :: { Member })
M.invitations = ui.state("settings_invitations", {} :: { Invitation })
M.status = ui.state("settings_status", {} :: { [string]: string })
-- Your photo as last heard, by whose it is: kept across launches so the
-- sidebar shows it before anything is asked (`M.photo()`).
local photo_cache = ui.persist("account_photo", { user = "", avatar = "" })

local function set_status(what: string, s: string)
	M.status:set_key(what, s)
end

function M.state(what: string): string
	return M.status:get()[what] or "loading"
end

-- The organization the session works in, by its id.
local function org_id(): string?
	local a = auth.account:peek() :: any
	return a.organization_id
end
-- Whether you can change the organization (rename it, invite, remove).
M.admin = ui.computed(function(): boolean
	local o = M.org:get()
	return o ~= nil and o.role == "admin"
end)

-- Each load's answers count only while it is the latest of its kind.
local latest: { [string]: number } = {}
local function ask(what: string): () -> boolean
	latest[what] = (latest[what] or 0) + 1
	local mine = latest[what]
	return function()
		return latest[what] == mine
	end
end

local function said(err: any): string
	return if type(err) == "table" or type(err) == "userdata" then err.message else tostring(err)
end

-- What a refusal says, said as what happened (each a key, read in the
-- locale shown when it's said).
local SAYS: { [string]: string } = {
	refused = "settings.store.says.refused",
	self = "settings.store.says.self",
	not_found = "settings.store.says.not_found",
	invalid = "settings.store.says.invalid",
	too_large = "settings.store.says.too_large",
	last_admin = "settings.store.says.last_admin",
	busy = "settings.store.says.busy",
	try_again = "settings.store.says.try_again",
	timed_out = "settings.store.says.timed_out",
	outdated = "settings.engine.outdated",
}
function M.says(word: string?): string
	local key = SAYS[word or ""]
	return if key then ui.t(key):get() else api.says(word)
end

local function failed(title: Message, err: any)
	toast.show { title = title:get(), body = M.says(said(err)), tone = "danger" }
end

-- Your profile as the edge answers it: kept, and its photo for the sidebar.
local function heard_me(me: Me)
	M.me:set(me)
	local u = auth.user:peek()
	if u then
		photo_cache:set({ user = u.id, avatar = me.avatar or "" })
	end
end

-- Your photo's address, the signed-in account's ("" none).
function M.photo(): string
	local u = auth.user:get()
	local cached = photo_cache:get()
	return if u and cached.user == u.id then cached.avatar else ""
end

-- Photos as files to show (a script's `ui.image` takes files only): each
-- address's local copy, fetched once by the host (`account.photo_file`) and
-- kept across launches; an address never changes what it shows.
local photo_files = ui.persist("account_photo_files", {} :: { [string]: string })
local fetching = ui.state("account_photo_fetching", {} :: { [string]: boolean })

-- `url`'s file ("" until it's here, or for none). Tracked.
function M.photo_file(url: string?): string
	if not url or url == "" then
		return ""
	end
	return photo_files:get()[url] or ""
end

local function want(url: string?)
	if not url or url == "" or photo_files:peek()[url] or fetching:peek()[url] then
		return
	end
	fetching:set_key(url, true)
	host.call("account.photo_file", { url = url }):after(function(path: string?)
		if path and path ~= "" then
			photo_files:set_key(url, path)
		end
	end, function() end)
end

-- The photos shown: yours, and your organization's members'.
ui.effect(function()
	want(M.photo())
	local me = M.me:get()
	want(me and me.avatar)
	for _, m in M.members:get() do
		want(m.avatar)
	end
end)

function M.load_me()
	local current = ask("me")
	if M.me:peek() == nil then
		set_status("me", "loading")
	end
	host.call("account.me"):after(function(me: any)
		if current() then
			heard_me(me)
			set_status("me", "ready")
		end
	end, function()
		if current() then
			set_status("me", "failed")
		end
	end)
end

-- Your username, then your card (none yet: no username).
function M.load_card()
	local current = ask("card")
	if M.card:peek() == nil then
		set_status("card", "loading")
	end
	api.username():after(function(name: string?)
		if not current() then
			return
		end
		M.username:set(name)
		if not name then
			M.card:set(nil)
			set_status("card", "ready")
			return
		end
		api.card(name):after(function(card: api.Card?)
			if current() then
				M.card:set(card)
				set_status("card", "ready")
			end
		end, function()
			if current() then
				set_status("card", "failed")
			end
		end)
	end, function()
		if current() then
			set_status("card", "failed")
		end
	end)
end

-- The organization worked in, from those you're in.
function M.load_org()
	local current = ask("org")
	if M.org:peek() == nil then
		set_status("org", "loading")
	end
	api.orgs():after(function(orgs: { api.Org })
		if not current() then
			return
		end
		local id = org_id()
		local found: Org? = nil
		for _, o in orgs do
			if o.organization_id == id or (id == nil and found == nil) then
				found = { organization_id = o.organization_id, name = o.name, role = o.role }
			end
		end
		M.org:set(found)
		set_status("org", "ready")
		if found then
			M.load_members()
			if found.role == "admin" then
				M.load_invitations()
			else
				M.invitations:set({})
				set_status("invitations", "ready")
			end
		end
	end, function()
		if current() then
			set_status("org", "failed")
		end
	end)
end

function M.load_members()
	local o = M.org:peek()
	if not o then
		return
	end
	local current = ask("members")
	if #M.members:peek() == 0 then
		set_status("members", "loading")
	end
	host.call("account.members", o.organization_id):after(function(list: any)
		if current() then
			M.members:set(list)
			set_status("members", "ready")
		end
	end, function()
		if current() then
			set_status("members", "failed")
		end
	end)
end

function M.load_invitations()
	local o = M.org:peek()
	if not o then
		return
	end
	local current = ask("invitations")
	host.call("account.invitations", o.organization_id):after(function(list: any)
		if current() then
			M.invitations:set(list)
			set_status("invitations", "ready")
		end
	end, function()
		if current() then
			set_status("invitations", "failed")
		end
	end)
end

-- Your name: shown at once, on your card too; back as it was if refused.
-- `done(ok)` once it's settled.
function M.rename(name: string, done: ((ok: boolean) -> ())?)
	local before = M.me:peek()
	local first, rest = string.match(name, "^%s*(%S+)%s*(.-)%s*$")
	M.me:set({
		first_name = first,
		last_name = if rest and rest ~= "" then rest else nil,
		name = name,
		email = if before then before.email else nil,
	})
	local card = M.card:peek()
	local card_before = card
	if card and first then
		local c = table.clone(card) :: any
		c.first_name = first
		M.card:set(c)
	end
	host.call("account.rename", name):after(function(me: any)
		heard_me(me)
		if done then
			done(true)
		end
	end, function(err: any)
		M.me:set(before)
		M.card:set(card_before)
		failed(ui.t("settings.store.couldnt_save_name"), err)
		if done then
			done(false)
		end
	end)
end

function M.rename_org(name: string, done: ((ok: boolean) -> ())?)
	local before = M.org:peek()
	if not before then
		return
	end
	local o = table.clone(before) :: any
	o.name = name
	M.org:set(o)
	host.call("account.rename_org", { organization_id = before.organization_id, name = name }):after(function()
		if done then
			done(true)
		end
	end, function(err: any)
		M.org:set(before)
		failed(ui.t("settings.store.couldnt_rename_org"), err)
		if done then
			done(false)
		end
	end)
end

local function index_of(list: { any }, id: string): number?
	for i, item in list do
		if item.id == id then
			return i
		end
	end
	return nil
end

-- Someone out of the organization: gone from the list at once, back in
-- their place if the edge refuses.
function M.remove(id: string)
	local o = M.org:peek()
	local at = index_of(M.members:peek(), id)
	if not o or not at then
		return
	end
	local member = M.members:peek()[at]
	M.members:remove(at)
	host.call("account.remove_member", { organization_id = o.organization_id, id = id }):after(function() end, function(err: any)
		local now = M.members:peek()
		M.members:insert(math.min(at, #now + 1), member)
		local who = member.name or member.email
		failed(if who then ui.t("settings.store.couldnt_remove", { name = who }) else ui.t("settings.store.couldnt_remove_them"), err)
	end)
end

function M.set_role(id: string, role: string)
	local o = M.org:peek()
	local at = index_of(M.members:peek(), id)
	if not o or not at then
		return
	end
	local before = M.members:peek()[at].role
	if before == role then
		return
	end
	M.members:set_key({ at, "role" }, role)
	host.call("account.set_role", { organization_id = o.organization_id, id = id, role = role }):after(function() end, function(err: any)
		local i = index_of(M.members:peek(), id)
		if i then
			M.members:set_key({ i, "role" }, before)
		end
		failed(ui.t("settings.store.couldnt_change_role"), err)
	end)
end

-- An invitation taken back: gone at once, back if refused.
function M.revoke(id: string)
	local o = M.org:peek()
	local at = index_of(M.invitations:peek(), id)
	if not o or not at then
		return
	end
	local invitation = M.invitations:peek()[at]
	M.invitations:remove(at)
	host.call("account.revoke_invite", { organization_id = o.organization_id, id = id }):after(function() end, function(err: any)
		local now = M.invitations:peek()
		M.invitations:insert(math.min(at, #now + 1), invitation)
		failed(ui.t("settings.store.couldnt_revoke"), err)
	end)
end

function M.resend(id: string)
	local o = M.org:peek()
	local at = index_of(M.invitations:peek(), id)
	if not o or not at then
		return
	end
	local email = M.invitations:peek()[at].email
	host.call("account.resend_invite", { organization_id = o.organization_id, id = id }):after(function()
		-- (No address while they're hidden on screen.)
		toast.show { title = ui.t("settings.store.invitation_resent"):get(), body = if PrivateText.hidden:peek() then nil else email, tone = "success" }
		M.load_invitations()
	end, function(err: any)
		failed(ui.t("settings.store.couldnt_resend"), err)
	end)
end

-- Invites `emails` as members: `done(invited)` with who was sent one and
-- who wasn't (account/api's Invited), or `done(nil, word)`.
function M.invite(emails: { string }, done: (invited: api.Invited?, word: string?) -> ())
	local o = M.org:peek()
	if not o then
		done(nil, "failed")
		return
	end
	api.invite(o.organization_id, emails):after(function(invited: api.Invited)
		M.load_invitations()
		done(invited, nil)
	end, function(err: any)
		done(nil, said(err))
	end)
end

-- Signed out: nothing of the account stays.
-- Your title (nil or "" clears it): shown at once, back as it was if
-- refused. `done(ok)` once it's settled.
function M.set_title(title: string?, done: ((ok: boolean) -> ())?)
	local before = M.me:peek()
	local t = if title and string.match(title, "%S") then string.match(title, "^%s*(.-)%s*$") else nil
	if before then
		local next = table.clone(before) :: any
		next.title = t
		M.me:set(next)
	end
	host.call("account.set_title", t or ""):after(function(me: any)
		heard_me(me)
		if done then
			done(true)
		end
	end, function(err: any)
		M.me:set(before)
		failed(ui.t("settings.store.couldnt_save_title"), err)
		if done then
			done(false)
		end
	end)
end

-- Your photo: the square `crop` ({ x, y, side }, in the picture's pixels) cut
-- from the picture at `path`, or none (`path` nil). `done(ok)` once settled.
function M.set_photo(path: string?, crop: { x: number, y: number, side: number }?, done: ((ok: boolean) -> ())?)
	local call = if path and crop
		then host.call("account.set_photo", { path = path, x = crop.x, y = crop.y, side = crop.side })
		else host.call("account.remove_photo")
	call:after(function(me: any)
		heard_me(me)
		if done then
			done(true)
		end
	end, function(err: any)
		failed(if path then ui.t("settings.store.couldnt_change_photo") else ui.t("settings.store.couldnt_remove_photo"), err)
		if done then
			done(false)
		end
	end)
end

-- Deletes your account (`confirm`, your address typed back). The engine
-- deletes it only just after you've signed in again (no agent on this Mac
-- can): asked for that, the browser opens on signing in again and
-- `signing_in()` is told, then the deletion lands once you have.
-- `done(word)`: nil once it's gone, else what stopped it ("last_admin",
-- "invalid", "cancelled", "timed_out", …).
function M.delete_account(confirm: string, done: (word: string?) -> (), signing_in: (() -> ())?)
	host.call("account.delete", { confirm = confirm }):after(function()
		done(nil)
	end, function(err: any)
		local word = said(err)
		if word ~= "step_up_required" then
			done(word)
			return
		end
		if signing_in then
			signing_in()
		end
		host.call("account.delete_signed_in", { confirm = confirm }):after(function()
			done(nil)
		end, function(again: any)
			done(said(again))
		end)
	end)
end

-- Stops waiting for that sign-in (its `done` says "cancelled").
function M.cancel_delete()
	host.invoke("account.cancel_step_up")
end

function M.forget()
	ui.batch(function()
		M.me:set(nil)
		M.card:set(nil)
		M.username:set(nil)
		M.org:set(nil)
		M.members:set({})
		M.invitations:set({})
		M.status:set({})
		photo_cache:set({ user = "", avatar = "" })
	end)
end

return M

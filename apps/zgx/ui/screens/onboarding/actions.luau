-- What onboarding does: move through the flow, sign in, apply the look,
-- finish.

local state = require("./state")
local auth = require("@account/auth")
local api = require("@account/api")
local material = require("@ds/material")
local palettes = require("@ds/palettes")

local M = {}

function M.go(step: state.Step)
	state.step:set(step)
end

-- Where `step` is in the active path.
local function index(step: state.Step): number?
	return table.find(state.order(), step)
end

-- On to the step after this one in the active path.
function M.next()
	local step = state.step:peek()
	M.finished(step)
	local order = state.order()
	-- From where it stands in the full path (a step that just left the
	-- path, naming the organization once the account turned out to be in
	-- one, still has its place there): the next step in the path. Every
	-- one in turn, finished before or not: only a run's start passes over
	-- what's finished (`start_run`).
	local past = false
	for _, s in state.PATHS[state.path:peek()] do
		if past and table.find(order, s) then
			state.step:set(s)
			return
		end
		past = past or s == step
	end
end

-- Back to the step before this one in the active path, never to claiming
-- a username or naming the organization: once passed, each is done for
-- good (a username is yours, an account makes one organization).
function M.back()
	local step = state.step:peek()
	local order = state.order()
	local i = index(step) or 1
	while i > 1 do
		i -= 1
		local s = order[i]
		-- (Nor to Sign in once signed in: leaving is Sign out.)
		local signin = s == "signin" and state.path:peek() == "signed_in"
		if s ~= "username" and s ~= "org" and not signin then
			state.step:set(s)
			return
		end
	end
end

-- Sign-in's two ways to an account, through the browser: "create" opens
-- its sign-up, "existing" its sign-in. `state.signin` is "waiting" until it
-- comes back, then `signed_in` or `sign_in_failed`; a sign-in given up
-- here goes back to ready. Clicking again while waiting starts afresh (the
-- first is given up).
-- Whether this load is the one waiting on the browser (and then on
-- Zeron's answers): a reload's new scripts hear nothing of an earlier
-- load's calls, so Sign in, still "waiting", is theirs to finish (init.luau
-- goes on once the account says signed in).
local asking = false
function M.asking(): boolean
	return asking
end

function M.start_sign_in(kind: state.Account)
	state.account:set(kind)
	state.signin:set("waiting")
	asking = true
	auth.sign_in(kind, function(user, failure)
		if not user then
			asking = false
		end
		if user then
			M.signed_in()
		elseif failure == "cancelled" then
			state.signin:set("ready")
		else
			M.sign_in_failed()
		end
	end)
end

-- Stops waiting on the browser (closed it, or changed their mind).
function M.cancel_sign_in()
	auth.cancel()
	asking = false
	state.signin:set("ready")
end

-- How long sign-in waits on Zeron's answers before going on without them
-- (each step that needs one asks again).
local ASK = 4000

-- Signed in (just now, or a session that was kept): the signed-in way
-- through. Three things are asked first, together: the pages the account
-- has finished (they leave the path), whether it has its username (then
-- claiming one isn't a step) and its organization (then it works in that
-- one). Then on to the first page left; with none left, setup is done.
-- Meanwhile Sign in stays as it was (or, on a launch, isn't drawn).
function M.signed_in()
	state.path:set("signed_in")
	state.claimed:set(false)
	state.card:set(nil)
	state.card_dealt:set(false)
	state.name_done:set(false)
	M.forget_org()
	local pending = 3
	local account_done: { string } = {}
	local went = false
	asking = true
	local function go()
		if went then
			return
		end
		went = true
		asking = false
		M.start_run(account_done)
		state.signin:set("ready")
	end
	local function answered()
		pending -= 1
		if pending == 0 then
			go()
		end
	end
	ui.after(ASK, go)
	api.onboarding():after(function(ids: { string })
		account_done = ids
		answered()
	end, answered)
	api.username():after(function(name: string?)
		state.name_done:set(name ~= nil)
		answered()
	end, answered)
	api.orgs():after(function(orgs: { api.Org })
		local org = M.existing_org(orgs)
		if org then
			M.work_in(org)
		end
		answered()
	end, answered)
end

-- A run starts: it opens on the first page not finished before it (the
-- account's, as Zeron answered, and this Mac's); with none left, on done.
function M.start_run(account_done: { string })
	local passed: { [string]: boolean } = {}
	if not state.replay then
		for _, id in account_done do
			passed[id] = true
		end
		for id, done in state.device_done:peek() do
			passed[id] = done
		end
	end
	state.passed:set(passed)
	local first: state.Step? = nil
	for _, s in state.order() do
		if s ~= "signin" and not passed[s] then
			first = s
			break
		end
	end
	-- None left: setup is done. (Until the shell exists there's nothing to
	-- open in setup's place, and a window closing at launch reads as a
	-- crash: it stands on You're all set instead.)
	state.step:set(first or "done")
end

-- A page left forwards is finished: on the account, or on this Mac.
function M.finished(step: state.Step)
	local scope = state.PAGES[step]
	if scope == "device" then
		local done = table.clone(state.device_done:peek())
		done[step] = true
		state.device_done:set(done)
	elseif scope == "account" and state.path:peek() == "signed_in" then
		api.onboarded({ step }):after(function() end, function() end)
	end
end

-- Of the organizations the account is in, the one to work in: the one
-- worked in last, else its own, else the first. Nil for none.
function M.existing_org(orgs: { api.Org }): api.Org?
	local last = state.last_org:peek()
	for _, o in orgs do
		if o.organization_id == last then
			return o
		end
	end
	for _, o in orgs do
		if o.role == "admin" then
			return o
		end
	end
	return orgs[1]
end

-- Works in the organization the account was already in: naming one leaves
-- the path, and inviting does too unless it's the account's own.
-- (Refused, the session stays as it was, and setup goes on regardless:
-- the sidebar can switch later.)
function M.work_in(org: api.Org)
	state.last_org:set(org.organization_id)
	state.org_name:set(org.name or "")
	state.org_admin:set(org.role == "admin")
	state.org_done:set(true)
	api.choose_org(org.organization_id):after(function() end, function() end)
end

-- No organization worked in this run (yet).
function M.forget_org()
	state.org_done:set(false)
	state.org_admin:set(false)
	state.org_name:set("")
	state.invited:set({})
end

-- Without an account: the look, then the agents.
function M.skip_sign_in()
	state.path:set("skipped")
	state.name_done:set(false)
	M.start_run({})
end

-- It didn't: the screen says so and its way on reads Try again
-- (`auth.says(auth.failure:get())` is the line for why).
function M.sign_in_failed()
	state.signin:set("failed")
end

-- The username is claimed: its card.
function M.claimed_username(card: api.Card)
	state.card:set(card)
	state.card_dealt:set(false)
	state.claimed:set(true)
	state.step:set("card")
end

-- The account had a username after all (the answer at sign-in came late,
-- or it was claimed elsewhere a moment ago): past claiming one, to its card.
function M.has_username()
	state.name_done:set(true)
	M.next()
end

-- Signed out (asked for, or the session went): back to the start.
function M.signed_out()
	state.wall_again:set(true)
	state.card:set(nil)
	state.card_dealt:set(false)
	state.signin:set("ready")
	state.path:set("skipped")
	state.name_done:set(false)
	state.claimed:set(false)
	M.forget_org()
	state.step:set("signin")
end

-- The organization just made, worked in from now on: on to inviting to it.
function M.org_made(organization_id: string, name: string)
	state.last_org:set(organization_id)
	state.org_name:set(name)
	state.org_admin:set(true)
	state.step:set("invite")
end

-- The colour scheme, as chosen: the theme, and the window's own chrome
-- (its traffic lights) in the same appearance.
function M.apply_scheme(mode: state.Appearance)
	if mode == "system" then
		ui.theme.follow_system { dark = palettes.scheme.dark, light = palettes.scheme.light }
		ui.invoke("appearance", ui.null)
	else
		-- (Through `current`, following the system off first: at zgx 06debe9
		-- `ui.theme.set` can be undone by the theme mirror's own effect on a
		-- fresh launch, leaving the window in the other scheme.)
		ui.theme.follow_system(false)
		ui.theme.current:set(if mode == "dark" then palettes.scheme.dark else palettes.scheme.light)
		ui.invoke("appearance", mode == "dark")
	end
end

function M.appearance(mode: state.Appearance)
	state.appearance:set(mode)
	M.apply_scheme(mode)
end

-- Frosted or opaque, for the whole app (ds.material).
M.frosted = material.frosted

-- Setup's done: what's new is marked seen for a run that went through
-- setup (news of what it just met), and the host opens the main window in
-- this one's place (remembering setup's done, so a launch signed in opens
-- there).
function M.finish()
	if state.path:peek() == "signed_in" and #state.NEWS > 0 then
		api.onboarded(state.NEWS):after(function() end, function() end)
	end
	ui.invoke("app.open_main")
end

return M

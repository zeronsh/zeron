-- Onboarding: a step at a time in setup's window (ds.Onboarding.Frame),
-- each step its own module with its footer, the numbered ones with their
-- dots. Two ways through (state.PATHS): signed in, the look, the account's
-- name and card, naming its organization and inviting to it, GitHub and
-- the agents; without an
-- account, the look and the agents. Sign-in opens both, and You're all set
-- ends them. A session that was kept starts on the look, an account that
-- already has its username passes over claiming one (its card still
-- shows), and one already in an organization works in it, past naming one
-- (and inviting, unless it's its own).

local ds = require("@ds")
local Onboarding = ds.Onboarding
local state = require("@self/state")
local actions = require("@self/actions")
local signin = require("@self/signin")
local auth = require("@account/auth")
local server = require("@screens/server")
local username = require("@self/username")
local card = require("@self/card")
local appearance = require("@self/appearance")
local org = require("@self/org")
local invite = require("@self/invite")
local github = require("@self/github")
local providers = require("@self/providers")
local done = require("@self/done")

-- The ways back and on. (The dots are the frame's, drawn once over every
-- step, so a step draws none of its own.)
local function nav(_step: state.Step): { [string]: any }
	return { on_back = actions.back, on_continue = actions.next }
end

-- While the intro hands over (the host's `onboarding.arrived` false), the
-- window holds opaque: the intro's copy of it, drawn over it, has no glass
-- under it, and the two must match until the copy goes.
local arrived = ui.shared("onboarding.arrived", true)
-- Sign in has been shown in this launch (signed out, or handed over).
local signin_seen = ui.state("signin_seen", false)
-- The server being chosen (its dialog open): from Sign in's pill, File →
-- Connect to Server…, or an invite (`zeron://server?address=…`).
local choosing_server = ui.state("signin_server", false)
-- The last `zeron://` link followed here (`app.link`'s seq).
local link_followed = ui.state("setup_link_followed", 0)

local STEPS: { [state.Step]: () -> Node } = {
	signin = function()
		-- A launch with a kept session goes straight to the look: Sign in
		-- (its card wall) isn't drawn while the Keychain is read, nor once
		-- it turns out signed in, until the step moves on. Once shown
		-- (signed out, or the intro's hand-off, whose copy shows it), it
		-- stays as a sign-in from it moves on.
		-- (Kept as state: a hot reload in the moment after a sign-in, Sign in
		-- still standing, keeps it standing.)
		ui.effect(function()
			local s = auth.state:get()
			if s == "signed_out" or s == "waiting" or not arrived:get() then
				signin_seen:set(true)
			end
		end)
		local hidden = ui.computed(function(): boolean
			local s = auth.state:get()
			local shown_once = signin_seen:get() or s == "signed_out" or s == "waiting" or not arrived:get()
			return not shown_once and (s == "restoring" or s == "signed_in")
		end)
		return ui.show(function()
			return not hidden:get()
		end, signin.screen {
			status = state.signin,
			on_create = function()
				actions.start_sign_in("create")
			end,
			on_existing = function()
				actions.start_sign_in("existing")
			end,
			on_cancel = actions.cancel_sign_in,
			says = function()
				return auth.says(auth.failure:get())
			end,
			on_skip = actions.skip_sign_in,
			-- (Zeron's goes unsaid; another is named over the title, the way
			-- to choosing again.)
			server = server.identity,
			on_server = ui.set(choosing_server, true),
		})
	end,
	appearance = function()
		return appearance(nav("appearance") :: any)
	end,
	username = function()
		local n = nav("username")
		return username {
			step = n.step,
			count = n.count,
			on_back = actions.back,
			on_not_now = actions.finish,
			on_claimed = actions.claimed_username,
			on_has_one = actions.has_username,
			on_signed_out = actions.signed_out,
		}
	end,
	card = function()
		local n = nav("card")
		return card { step = n.step, count = n.count, on_back = actions.back, on_continue = actions.next }
	end,
	org = function()
		return org(nav("org") :: any)
	end,
	invite = function()
		return invite(nav("invite") :: any)
	end,
	github = function()
		return github(nav("github") :: any)
	end,
	providers = function()
		return providers(nav("providers") :: any)
	end,
	done = function()
		return done(nav("done") :: any)
	end,
}


return function(): Node
	-- A launch that finds you signed in (the session restored from the
	-- Keychain) has no Sign in to show: on past it, as a sign-in would go.
	-- Not while the intro hands over: its copy of this window, which has no
	-- account, shows Sign in, and the two must match until it's gone; the
	-- step moves on as the window becomes yours.
	-- Once, and outside the effect: what it starts (a call to Zeron) is no
	-- dependency of it. The same after a hot reload in the middle of a
	-- sign-in (Sign in still waiting on the browser, or on Zeron's
	-- answers): the load that asked is gone, and with it what it would
	-- have done with the answers, so this load asks again.
	local skipped = false
	ui.effect(function()
		if skipped or auth.state:get() ~= "signed_in" or not arrived:get() then
			return
		end
		skipped = true
		ui.after(0, function()
			if state.step:peek() ~= "signin" then
				return
			end
			local s = state.signin:peek()
			if s == "ready" or (s == "waiting" and not actions.asking()) then
				actions.signed_in()
			end
		end)
	end)
	-- The scheme chosen, from the start and as it's picked (Make it yours
	-- sets it; every step is seen in it).
	ui.effect(function()
		actions.apply_scheme(state.appearance:get())
	end)
	-- File → Connect to Server…, from any step (Sign in's pill is the way
	-- on its own page). Not while a launch's environment fixes the server.
	if auth.server:peek().kind ~= "environment" then
		ui.command {
			id = "setup.connect_server",
			title = ui.t("onboarding.connect_server"):get(),
			menu = "File",
			run = function()
				server.ask()
			end,
		}
	end
	-- An invite clicked (`zeron://server?address=…`, read by the host,
	-- src/links.rs): the dialog, on its address, once the window is its own
	-- (not over the intro's hand-off). Followed once each, never on a
	-- reload.
	local link = ui.shared("app.link", { seq = 0 } :: { seq: number, server: string? })
	ui.effect(function()
		local l = link:get()
		if l.seq == 0 or l.seq == link_followed:peek() or not arrived:get() then
			return
		end
		link_followed:set(l.seq)
		if l.server then
			server.ask(l.server)
		end
	end)
	-- Another server chosen. Signed out of one: back to the start, on the
	-- new one. From past Sign in (setup without an account): back to it, to
	-- sign in there. On Sign in: a failure said for the last one, or a
	-- sign-in waiting on its browser, no longer stands.
	local function server_changed(signed_out: boolean)
		if signed_out or state.step:peek() ~= "signin" then
			actions.signed_out()
		elseif state.signin:peek() == "waiting" then
			actions.cancel_sign_in()
		elseif state.signin:peek() == "failed" then
			state.signin:set("ready")
		end
	end
	return Onboarding.Frame {
		opaque = function()
			return not arrived:get()
		end,
		steps = {
			count = function(): number
				local _, count = state.position(state.step:get())
				return count
			end,
			current = function(): number?
				local n = state.position(state.step:get())
				return n
			end,
		},
		ui.presence(state.step, function(s: state.Step)
			local build = STEPS[s] or STEPS.signin
			return build()
		end, {
			w_full = true,
			h_full = true,
			mode = "wait",
			enter = { opacity = { 0, 1 }, y = { 12, 0 }, blur = { 8, 0 }, duration = 420, ease = "outExpo" },
			exit = { opacity = 0, y = -8, duration = 160, ease = "inQuad" },
		}),
		-- (Over every step: an invite or the menu opens it wherever setup is.)
		server.Dialog { open = choosing_server, on_changed = server_changed },
	}
end

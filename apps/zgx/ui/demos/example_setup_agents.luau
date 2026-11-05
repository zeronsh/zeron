-- Setup, this device's last numbered step: the coding agents on this Mac,
-- in Settings → Providers' own cards (demos/providers_parts.luau, shared
-- with that page so the two can't drift), as the real step embeds that
-- page (onboarding.rs render_providers). Each agent a card: its mark,
-- version and switch, its accounts under it with their plan and usage, and
-- at its foot Add account (or Connect a … account for the first), so more
-- than one account can be connected here exactly as in Settings. A sign-in
-- runs in the foot's row: waiting on the browser, or didn't finish. What
-- Settings alone has stays there: the device select, each agent's settings.
--
-- The words over the list are the real step's ("Found 4 agents on this
-- Mac. Sign in to any you'd like to use."; "Install an agent" when there
-- is none). The title holds still and the list scrolls in a fixed room
-- under it. Its states are the real step's: looking, found, a sign-in
-- waiting or failed, none installed (the agents to install, each a click;
-- Skip for now, as the real step allows, since the rest of the app works
-- without one), an install under way, and this Mac couldn't be checked
-- (Skip for now again).

local ds = require("@ds")
local kit = require("@demos/example_kit")
local parts = require("@demos/providers_parts")
local setup = require("@demos/setup_parts")

local shown = ui.state("ex_sa_state", "found")

-- Each agent's card foot on this step: "idle" (Add account), a sign-in
-- "waiting", or one that "failed".
local login: { [string]: State<string> } = {}
for _, p in parts.INSTALLED do
	login[p.id] = ui.state(`ex_sa_login_{p.id}`, "idle")
end

-- The real setup's column (onboarding.rs COLUMN_WIDTH): an account's row
-- holds its meters beside its words at this width, not at a list's 480.
local MEASURE = 560

local function none_installed(): boolean
	local s = shown:get()
	return s == "none" or s == "installing"
end

local function checked(): boolean
	return none_installed() or shown:get() == "unavailable"
end

-- The found agents' cards, each foot following its sign-in in place: the
-- cards are built once (as the real step's keyed rows are).
local function cards(): Node
	local list: { Node } = {}
	for _, p in parts.INSTALLED do
		local foot = login[p.id]
		table.insert(list, parts.card(p, function()
			return foot:get()
		end, {
			settings = false,
			on_add = function()
				foot:set("waiting")
			end,
			settle = function(next: string)
				foot:set(next)
			end,
		}))
	end
	return ui.column { w_full = true, gap = "$space.4", table.unpack(list) }
end

-- Nothing on this Mac: Zeron's own agents, a click each; Claude Code's
-- install runs through the Installing state.
local function install(): Node
	local rows: { Node } = {}
	local all: { parts.Agent } = table.clone(parts.INSTALLED)
	for _, p in parts.MORE do
		table.insert(all, p)
	end
	for _, p in all do
		table.insert(rows, parts.install_row(p, function()
			return p.id == "claude" and shown:get() == "installing"
		end, function()
			if p.id == "claude" then
				shown:set("installing")
			end
		end))
	end
	return ds.Settings.Group(rows :: any)
end

local function body(k: string): Node
	if k == "looking" then
		return parts.loading()
	elseif k == "unavailable" then
		return parts.unavailable(function()
			return "this Mac"
		end, function()
			shown:set("looking")
			ui.after(900, function()
				if shown:peek() == "looking" then
					shown:set("found")
				end
			end)
		end)
	elseif k == "install" then
		return install()
	end
	return cards()
end

return {
	id = "ex_setup_agents",
	title = "Your agents",
	about = "This device's last numbered setup step, on Settings → Providers' own cards: each agent with its version and switch, its accounts with plan and usage, and Add account at its foot, so several can be connected here as in Settings. A sign-in waiting or failed in the foot's row; looking, none installed (install one, or skip), an install under way, and this Mac couldn't be checked. The list scrolls under a title that holds still.",
	demo = function()
		ui.effect(function()
			local s = shown:get()
			if s == "found" or s == "signing" or s == "failed" then
				for id, st in login do
					st:set(if id ~= "cursor" then "idle" elseif s == "signing" then "waiting" elseif s == "failed" then "failed" else "idle")
				end
			end
		end)
		local function step(count: number): Node
			-- The words and the cards scroll together, the title giving way to a
			-- small one in the top (the app's step's, ds.Onboarding.ScrollStep).
			return kit.scroll_step {
				step = count,
				count = count,
				back = kit.back {},
				aside = ui.show(checked, kit.quiet { label = "Skip for now" }),
				continue = {
					label = "Continue",
					disabled = function()
						return shown:get() == "looking" or checked()
					end,
				},
				form = {
					measure = MEASURE,
					-- Nothing to run yet: the title asks for one (the real step's).
					title = function()
						return if none_installed() then "Install an agent" else "Your agents"
					end :: any,
					-- (One line in every state, so the title never moves.)
					body = function()
						local s = shown:get()
						if s == "looking" then
							return "Looking for coding agents on this Mac…"
						elseif s == "unavailable" then
							return "Zeron runs the coding agents you already use."
						elseif none_installed() then
							return "None are on this Mac yet. Install one to continue."
						end
						return `Found {#parts.INSTALLED} agents on this Mac. Sign in to any you'd like to use.`
					end,
				},
				ui.presence(function(): string
					local s = shown:get()
					return if s == "looking" or s == "unavailable" then s elseif none_installed() then "install" else "found"
				end, body, { w_full = true, enter = { opacity = { 0, 1 }, duration = 140 }, exit = { opacity = 0, duration = 80 }, mode = "wait" }),
			}
		end
		return kit.stage {
			states = {
				{ value = "looking", label = "Looking" },
				{ value = "found", label = "Found" },
				{ value = "signing", label = "Signing in" },
				{ value = "failed", label = "Sign-in failed" },
				{ value = "none", label = "None installed" },
				{ value = "installing", label = "Installing" },
				{ value = "unavailable", label = "Couldn't check" },
			},
			state = shown,
			also = setup.account_switch(),
			w = 680,
			h = 500,
			-- An account's ⋯ menu offers a banked reset and removing it: their
			-- dialogs, Settings' own.
			overlays = parts.dialogs(),
			setup.counted(step),
		}
	end,
}

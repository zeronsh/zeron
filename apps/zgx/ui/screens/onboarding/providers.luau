-- The coding agents on this Mac, the last numbered step either way, in
-- Settings → Providers' own cards (ds.ProviderCard): each agent with its
-- version and switch, its accounts with their plan and usage, and at its
-- foot the way to connect one, so several can be connected here as in
-- Settings. The gallery's Your agents is the design, and its states are
-- this step's: looking, found, a sign-in waiting or failed (in the card's
-- foot), none installed (the agents to install, each a click; Skip for
-- now, since the rest of the app works without one), an install under way,
-- and this Mac couldn't be checked (Skip for now again). The words and the
-- cards scroll together, the title giving way to a small one in the top.
--
-- What's found is the engine's (account/agents). The cards are a keyed
-- `ui.each` whose rows get their agent as state: an agent's card is built
-- once and changes in place as its switch flips, its sign-in starts and
-- ends, its accounts come and go and their usage moves, so the button just
-- pressed stays, and the page keeps its scroll (tests/providers_stability).
-- Only the step's own states swap (`presence`), as only they replace what
-- shows.

local ds = require("@ds")
local agents = require("@account/agents")
local state = require("./state")
local Onboarding = ds.Onboarding

export type Props = { step: number?, count: number?, on_back: () -> (), on_continue: () -> () }

local function now(): agents.Now
	return agents.now:get() :: agents.Now
end

-- The agents shown: those with a mark (every one Zeron runs but a test rig).
local function shown(): { agents.Agent }
	local out = {}
	for _, a in now().list do
		if a.mark then
			table.insert(out, a)
		end
	end
	return out
end

local function installed(): { agents.Agent }
	local out = {}
	for _, a in shown() do
		if a.installed then
			table.insert(out, a)
		end
	end
	return out
end

local function by_id(a: agents.Agent): string
	return a.id
end

-- Which of the step's states it's in.
local function kind(): string
	local s = now().state
	if s == "looking" or s == "unavailable" then
		return s
	end
	return if #installed() == 0 then "install" else "found"
end

-- Nothing to sign into: the way on is Skip for now.
local function checked(): boolean
	local k = kind()
	return k == "install" or k == "unavailable"
end

-- An agent's card, from its row of the list (`row`, the agent as it
-- stands): built once, everything on it read from the row.
local function card(row: State<agents.Agent>): Node
	local first = row:peek()
	local id = first.id
	-- The switch is the card's own, for as long as the card stands. It
	-- follows the engine, and a flip goes to the engine, whose answer comes
	-- back in the row.
	local on = ui.use_state(first.enabled)
	local engine_said = first.enabled
	ui.effect(function()
		local said, flipped = row:get().enabled, on:get()
		if said ~= engine_said then
			engine_said = said
			if flipped ~= said then
				on:set(said)
			end
		elseif flipped ~= said then
			agents.set_enabled(id, flipped)
		end
	end)
	return ds.ProviderCard.Card {
		provider = { id = id, name = first.name, mark = first.mark :: string },
		version = function()
			return row:get().version
		end,
		accounts = function()
			return row:get().accounts :: any
		end,
		foot = function()
			return row:get().login
		end,
		enabled = on,
		active = function()
			for _, acc in row:get().accounts do
				if acc.active then
					return acc.email
				end
			end
			return nil
		end,
		settings = false,
		on_add = function()
			agents.sign_in(id)
		end,
		on_code = function(code: string)
			agents.complete(id, code)
		end,
		settle = function(next: string)
			local login = row:peek().login
			if next == "waiting" then
				agents.sign_in(id)
			elseif login == "waiting" or login == "code" then
				agents.cancel(id)
			else
				agents.dismiss(id)
			end
		end,
		on_switch = function(acc)
			if acc.id then
				agents.switch(id, acc.id)
			end
		end,
		on_remove = function(acc)
			if acc.id then
				agents.remove(id, acc.id)
			end
		end,
	}
end

local function cards(): Node
	return ui.column {
		w_full = true,
		gap = "$space.4",
		ui.each(ui.computed(installed), function(_a: agents.Agent, _i: number, row: State<agents.Agent>): Node
			return card(row)
		end, { key = by_id }),
	}
end

-- Nothing on this Mac: the agents Zeron can install, a click each; one it
-- can't says to install it yourself. A row each, keyed, its install under
-- way read from its row; each brings the rule above it but the first.
local function install(): Node
	local function top(): string?
		local list = shown()
		return if list[1] then list[1].id else nil
	end
	return ds.Settings.Group {
		ui.column {
			w_full = true,
			ui.each(ui.computed(shown), function(_a: agents.Agent, _i: number, row: State<agents.Agent>): Node
				local a = row:peek()
				return ui.column {
					w_full = true,
					ui.show(function()
						return top() ~= a.id
					end, ds.Settings.Rule()),
					ds.ProviderCard.InstallRow({ name = a.name, mark = a.mark :: string, manual = not a.can_install }, function()
						return row:get().installing
					end, function()
						agents.install(a.id)
					end),
				}
			end, { key = by_id }),
		},
	}
end

local function body(k: string): Node
	if k == "looking" then
		return ds.ProviderCard.Loading()
	elseif k == "unavailable" then
		return ds.ProviderCard.Unavailable(function()
			return ui.t("onboarding.providers.this_mac"):get()
		end, agents.load)
	elseif k == "install" then
		return install()
	end
	return cards()
end

return function(props: Props): Node
	-- (Once a visit: a hot reload doesn't ask again.)
	state.once_a_visit("agents", agents.load)
	-- The words and the cards scroll together (ds.Onboarding.ScrollStep).
	return Onboarding.ScrollStep {
		-- (A reload puts its titles back as they were.)
		place = "providers",
		step = props.step,
		count = props.count,
		back = Onboarding.Back { on_click = props.on_back },
		aside = ui.show(checked, Onboarding.Quiet { label = ui.t("onboarding.skip_for_now") :: any, on_click = props.on_continue }),
		continue = {
			label = ui.t("common.continue") :: any,
			disabled = function()
				return kind() == "looking" or checked()
			end,
			on_click = props.on_continue,
		},
		form = {
			measure = Onboarding.LIST_MEASURE,
			-- Nothing to run yet: the title asks for one.
			title = function()
				local k = kind()
				return if k == "install" then ui.t("onboarding.providers.title_install"):get() else ui.t("onboarding.providers.title"):get()
			end :: any,
			-- (One line in every state, so the title never moves.)
			body = function()
				local k = kind()
				if k == "looking" then
					return ui.t("onboarding.providers.looking"):get()
				elseif k == "unavailable" then
					return ui.t("onboarding.providers.unavailable"):get()
				elseif k == "install" then
					return ui.t("onboarding.providers.none"):get()
				end
				return ui.t("onboarding.providers.found", { n = #installed() }):get()
			end,
		},
		-- The step's own states: these alone swap what shows.
		ui.presence(kind, body, { w_full = true, enter = { opacity = { 0, 1 }, duration = 140 }, exit = { opacity = 0, duration = 80 }, mode = "wait" }),
	}
end

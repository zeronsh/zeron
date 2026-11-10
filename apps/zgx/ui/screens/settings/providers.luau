-- Settings → Providers: the coding agents on this Mac and the accounts
-- they sign in with, the gallery's Providers (demos/example_providers) on
-- the engine's data (account/agents): each installed agent a card in its
-- settings form (its version and Update, its settings, its switch), its
-- accounts with their plans and usage and the way to add one; then the
-- agents still to install, a click each. Setup's Your agents shows the same
-- cards (screens/onboarding/providers.luau).
--
-- The cards are a keyed `ui.each` whose rows get their agent as state: a
-- card is built once and changes in place as its switch flips, a sign-in
-- starts and ends, accounts come and go and an update arrives. Only the
-- page's own states swap (looking, none here, can't be checked).

local host = require("@app/screens/settings/host")
local ds = require("@ds")
local agents = require("@account/agents")
local toast = require("@ds/toast")
local search = require("@app/screens/settings/search")
local devices_page = require("@app/screens/settings/devices")
local sessions = require("@account/sessions")
local visits = require("@app/screens/settings/visits")
local prefs = require("@app/screens/settings/prefs")
local registry = require("@account/registry")
local add_agent = require("@app/screens/settings/add_agent")
local sign_in_terminal = require("@app/screens/settings/sign_in_terminal")

local M = {}

M.ID = "providers"
M.TITLE = "Providers"

search.index(M.ID, ui.t("settings.providers.title"), ui.t("settings.providers.terms"), {
	{ key = "agents", title = ui.t("settings.providers.agents.label"), terms = ui.t("settings.providers.agents.terms") },
	{
		key = "native_subagents",
		title = ui.t("settings.providers.native_subagents.label"),
		section = ui.t("settings.providers.agent_settings"),
		terms = ui.t("settings.providers.native_subagents.terms"),
	},
	{
		key = "update_policy",
		title = ui.t("settings.providers.update_policy.label"),
		section = ui.t("settings.providers.agent_settings"),
		terms = ui.t("settings.providers.update_policy.terms"),
	},
	{
		key = "skills_dollar",
		title = ui.t("settings.providers.skills_dollar.label"),
		section = ui.t("settings.providers.agent_settings"),
		terms = ui.t("settings.providers.skills_dollar.terms"),
	},
	{
		key = "skills_separate",
		title = ui.t("settings.providers.skills_separate.label"),
		section = ui.t("settings.providers.agent_settings"),
		terms = ui.t("settings.providers.skills_separate.terms"),
	},
	{ key = "more", title = ui.t("settings.providers.more.label"), terms = ui.t("settings.providers.more.terms") },
})

local function now(): agents.Now
	return agents.now:get() :: agents.Now
end

-- The agents shown: those with a mark (every one Zeron runs but a test rig),
-- and those added from the ACP registry.
local function shown(): { agents.Agent }
	local out = {}
	for _, a in now().list do
		if a.mark or (a :: any).registry then
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

local function missing(): { agents.Agent }
	local out = {}
	for _, a in shown() do
		if not a.installed then
			table.insert(out, a)
		end
	end
	return out
end

-- Whose agents show: one of the account's computers (agents run on a Mac
-- or Linux, not a phone), this one first. Choosing one asks the engine to
-- address every agent call there (`agents.target`).
local function computers(): { sessions.Device }
	return sessions.computers(sessions.devices:get(), true)
end
local function here(): string?
	for _, d in sessions.now:peek().devices or {} do
		if d.here then
			return d.id
		end
	end
	return nil
end
-- The device shown: the engine's target, else this one.
local function shown_device(): string?
	return now().target or here()
end
-- (In the locale shown: read where it's drawn.)
local function device_name(): string
	local id = shown_device()
	for _, d in computers() do
		if d.id == id then
			return if d.here then ui.t("settings.providers.this_mac"):get() else d.name
		end
	end
	return ui.t("settings.providers.this_mac"):get()
end
local function choose_device(id: string)
	agents.target(if id == here() then nil else id)
	-- (The registry as that device sees it: what it has added, its builds.)
	registry.load()
end

-- Which of the page's views shows: the agents, the registry ("add"), or
-- one agent of it ("agent:<id>").
local view = ui.state("settings_providers_view", "agents")
local function go(v: string)
	local id = string.match(v, "^agent:(.+)$")
	if id then
		add_agent.visit(id)
	end
	view:set(v)
	if v ~= "agents" then
		registry.load()
	end
end
local nav = { go = go, device = device_name }

local function by_id(a: agents.Agent): string
	return a.id
end

local function kind(): string
	local s = now().state
	if s == "looking" or s == "unavailable" then
		return s
	end
	return "found"
end

-- Which agent's settings are open (none: "").
local open_settings = ui.state("settings_agent_open", "")
-- The engine's native subagent settings: those it can switch, and those off.
local subagents = ui.state("settings_subagents", nil :: { switchable: { string }, off: { string } }?)

local function load_subagents()
	host.call("settings.engine", { method = "GetNativeSubagents", params = {} }):after(function(v: any)
		subagents:set({ switchable = v.switchable or {}, off = v.off or {} })
	end, function() end)
end

-- The Codex account whose banked resets are being chosen from (by its
-- id), and the reset chosen. The account is read from the list as it
-- stands, so its resets are the device's; it's spent there (account/agents).
local reset_open = ui.state("settings_reset_open", false)
local reset_for = ui.state("settings_reset_for", "")
local reset_choice = ui.state("settings_reset_choice", "")

local function reset_account(): agents.Account?
	local id = reset_for:get()
	if id == "" then
		return nil
	end
	for _, a in now().list do
		for _, acc in a.accounts do
			if acc.id == id then
				return acc
			end
		end
	end
	return nil
end

local function open_resets(account: string?)
	if account then
		reset_for:set(account)
		reset_open:set(true)
	end
end

local function resets_dialog(): Node
	-- (The account gone, another device shown: nothing left to choose.)
	ui.effect(function()
		if reset_open:get() and reset_account() == nil then
			reset_open:set(false)
		end
	end)
	return ds.ProviderCard.ResetsDialog {
		open = reset_open,
		account = reset_account :: any,
		choice = reset_choice,
		on_use = function(acc, credit: string)
			if acc.id then
				agents.use_reset(acc.id, credit)
			end
		end,
	}
end

-- An agent's card, from its row of the list (`row`, the agent as it
-- stands): built once, everything on it read from the row.
local function card(row: State<agents.Agent>): Node
	local first = row:peek()
	local id = first.id
	-- An agent added from the ACP registry (or one of your own): it keeps
	-- no accounts, and signs in its own way. (Its updates come as any
	-- agent's: the engine's update coordinator, the registry its feed.)
	local from_registry = (first :: any).registry == true
	-- The switch follows the engine; a flip goes to it, and its answer
	-- comes back in the row.
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
		provider = { id = id, name = first.name, mark = first.mark, icon = (first :: any).icon },
		version = function()
			return row:get().version
		end,
		latest = function()
			return (row:get() :: any).latest
		end,
		updating = function()
			return (row:get() :: any).updating == true
		end,
		sign_in_only = from_registry,
		login_note = function(): string?
			return (row:get() :: any).login_note
		end,
		-- One that asks for typed input: its sign-in, in a terminal on the
		-- agent's device.
		terminal = function(): boolean
			return (row:get() :: any).login_command ~= nil
		end,
		login_page = function(): boolean
			return (row:get() :: any).login_page ~= nil
		end,
		on_open_page = function()
			agents.open_page(id)
		end,
		login_code = function(): string?
			return (row:get() :: any).login_code
		end,
		signed_in = function(): boolean?
			return (row:get() :: any).signed_in
		end,
		signed_in_as = function(): string?
			return (row:get() :: any).signed_in_as
		end,
		can_sign_out = function(): boolean
			return (row:get() :: any).can_sign_out == true
		end,
		signing_out = function(): boolean
			return (row:get() :: any).signing_out == true
		end,
		on_sign_out = function()
			agents.sign_out(id)
		end,
		login_methods = function()
			return (row:get() :: any).login_methods or {}
		end,
		on_sign_in_with = function(method: string)
			agents.sign_in_with(id, method)
		end,
		on_key = function(var: string, value: string)
			agents.set_key(id, var, value)
		end,
		on_terminal = function()
			local command = (row:peek() :: any).login_command
			if command then
				sign_in_terminal.show {
					id = id,
					name = first.name,
					command = command,
					device = now().target,
					device_name = device_name(),
				}
			end
		end,
		on_update = function()
			host.call("agents.update", id):after(function() end, function(err: any)
				toast.show {
					title = ui.t("settings.providers.couldnt_update", { name = first.name }):get(),
					body = if err.message == "offline"
						then ui.t("settings.providers.cant_reach", { device = device_name() }):get()
						else ui.t("settings.providers.try_later"):get(),
					tone = "danger",
				}
			end)
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
		settings = true,
		on_settings = function()
			load_subagents()
			open_settings:set(id)
		end,
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
		-- A Codex account's banked resets: the dialog asks which; how it
		-- went lands on the account's row.
		on_reset = function(acc)
			open_resets(acc.id)
		end,
		on_dismiss_reset = function(acc)
			if acc.id then
				agents.dismiss_reset(acc.id)
			end
		end,
	}
end

local function cards(): Node
	return search.anchor(
		"agents",
		ui.column {
			w_full = true,
			gap = "$space.4",
			ui.each(ui.computed(installed), function(_a: agents.Agent, _i: number, row: State<agents.Agent>): Node
				return card(row)
			end, { key = by_id }),
		}
	)
end

-- The agents still to install: Zeron's own, a click each; one it can't
-- install says to install it yourself. Each brings the rule above it but
-- the first.
local function more(): Node
	local list = ui.computed(missing)
	local function top(): string?
		local l = list:get()
		return if l[1] then l[1].id else nil
	end
	-- Every other agent there is: the registry's, a page away (its count
	-- once it's known).
	local function others(): number
		local n = 0
		for _, a in registry.now:get().agents do
			if not a.built_in and not a.delisted and not a.added then
				n += 1
			end
		end
		return n
	end
	return search.anchor(
		"more",
		ds.Settings.Section {
			title = ui.t("settings.providers.more.label"),
			description = ui.t("settings.providers.more.description"),
			ds.Settings.Group {
				ui.column {
					w_full = true,
					ui.each(list, function(_a: agents.Agent, _i: number, row: State<agents.Agent>): Node
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
					ui.show(function()
						return #list:get() > 0
					end, ds.Settings.Rule()),
					ds.Settings.Row {
						label = ui.t("settings.providers.add.row_label"),
						description = function(): string
							local n = others()
							return if n > 0
								then ui.t("settings.providers.add.row_description", { n = n }):get()
								else ui.t("settings.providers.add.row_description_none"):get()
						end,
						lead = ds.ProviderCard.disc(ds.Icon { name = "plus", size = 16 }),
						on_click = function()
							go("add")
						end,
					},
				},
			},
		}
	)
end

local function body(k: string): Node
	if k == "looking" then
		return ds.ProviderCard.Loading()
	elseif k == "unavailable" then
		return ds.ProviderCard.Unavailable(function()
			return device_name()
		end, agents.load)
	end
	return ui.column { w_full = true, gap = "$space.6", cards(), more() }
end

-- How an agent's updates come, and what each means (in the locale shown:
-- read where they're drawn).
local function POLICIES(): { any }
	return {
		{ value = "notify", label = ui.t("settings.providers.policy.notify"):get() },
		{ value = "auto-when-idle", label = ui.t("settings.providers.policy.auto"):get() },
		{ value = "off", label = ui.t("settings.providers.policy.off"):get() },
	}
end
local POLICY_NOTE: { [string]: string } = {
	notify = "settings.providers.policy.notify_note",
	["auto-when-idle"] = "settings.providers.policy.auto_note",
	off = "settings.providers.policy.off_note",
}

-- The open agent's settings, in a dialog from its card: whether it uses
-- its own subagents, and how its updates are taken. They apply as they're
-- changed, so the dialog only needs a way out.
local dialog_open = ui.computed(function(): boolean
	return open_settings:get() ~= ""
end)
local function agent(): agents.Agent?
	local id = open_settings:get()
	for _, a in now().list do
		if a.id == id then
			return a
		end
	end
	return nil
end
local function settings_dialog(): Node
	local open = ui.state("settings_agent_dialog", false)
	ui.effect(function()
		local want = dialog_open:get()
		if want ~= open:peek() then
			open:set(want)
		end
	end)
	ui.effect(function()
		if not open:get() and open_settings:peek() ~= "" then
			open_settings:set("")
		end
	end)
	local native = ui.state("settings_agent_native", true)
	local policy = ui.state("settings_agent_policy", "notify")
	ui.effect(function()
		local a = agent()
		local s = subagents:get()
		if a then
			native:set(not (s ~= nil and table.find(s.off, a.id) ~= nil))
			policy:set((a :: any).update_policy or "notify")
		end
	end)
	-- Its completion in the composer (this Mac's, prefs.completion: the
	-- composer reads it by the chat's agent).
	local dollar = ui.state("settings_agent_dollar", true)
	local separate = ui.state("settings_agent_separate", true)
	ui.effect(function()
		local a = agent()
		if a then
			local c = prefs.completion(a.id)
			dollar:set(c.dollar)
			separate:set(c.separate)
		end
	end)
	local function completion(key: "dollar" | "separate"): (on: boolean) -> ()
		return function(on: boolean)
			local a = agent()
			if a then
				prefs.set_completion(a.id, key, on)
			end
		end
	end
	local function switchable(): boolean
		local a, s = agent(), subagents:get()
		return a ~= nil and s ~= nil and table.find(s.switchable, a.id) ~= nil
	end
	-- One added from the ACP registry (or of your own): it can be removed
	-- from the device, and has an environment there. One of your own isn't
	-- updated by Zeron: no policy.
	local function from_registry(): boolean
		local a = agent()
		return a ~= nil and (a :: any).registry == true
	end
	local function own(): boolean
		local a = agent()
		return a ~= nil and string.sub(a.id, 1, 8) == "acp.own."
	end
	local function remove()
		local a = agent()
		local l = if a then registry.of_harness(a.id) else nil
		if not (a and l) then
			return
		end
		local name = a.name
		open:set(false)
		registry.remove(l.id):after(function() end, function()
			toast.show { title = ui.t("settings.providers.add.couldnt_remove", { name = name }):get(), tone = "danger" }
		end)
	end
	-- Its environment on the device, as it was last kept (`kept`), and as
	-- typed: kept again a moment after typing stops, or why a line isn't
	-- one. (Never in state that syncs: the host's call, the device's file.)
	local env = ui.state("settings_agent_env", "")
	local env_error = ui.state("settings_agent_env_error", "")
	local kept = ""
	local typed = 0
	local function registry_id(): string?
		local a = agent()
		local l = if a and (a :: any).registry then registry.of_harness(a.id) else nil
		return if l then l.id else nil
	end
	ui.effect(function()
		local id = registry_id()
		if not id or not dialog_open:get() then
			return
		end
		kept = ""
		env:set("")
		env_error:set("")
		registry.env_get(id):after(function(text: string)
			kept = text
			env:set(text)
		end, function() end)
	end)
	ui.effect(function()
		local text = env:get()
		local id = registry_id()
		if not id or text == kept then
			return
		end
		typed += 1
		local mine = typed
		ui.after(700, function()
			if mine ~= typed or env:peek() ~= text then
				return
			end
			registry.env_set(id, text):after(function(saved: string)
				kept = text
				env_error:set("")
				local _ = saved
			end, function(err: any)
				env_error:set(if type(err) == "table" and err.message ~= "offline" then err.message else ui.t("settings.engine.couldnt_save"):get())
			end)
		end)
	end)
	return ds.Dialog {
		open = open,
		title = ui.t("settings.providers.agent_settings"),
		body = (function(): string
			return ui.t("settings.providers.dialog_body", { device = device_name() }):get()
		end) :: any,
		size = "md",
		cancel = false,
		confirm = { label = ui.t("settings.providers.done") :: any },
		ds.Settings.Group {
			ui.div {
				w_full = true,
				opacity = function()
					return if switchable() then 1 else "$zeron.disabled"
				end,
				disabled = function()
					return not switchable()
				end,
				ds.Settings.Row {
					label = ui.t("settings.providers.native_subagents.label"),
					description = ui.t("settings.providers.native_subagents.description"),
					control = ds.Switch {
						value = native,
						label = ui.t("settings.providers.native_subagents.label"),
						on_change = function(on: boolean)
							local a = agent()
							if not a then
								return
							end
							host.call("settings.engine", { method = "SetNativeSubagents", params = { harness = a.id, enabled = on } }):after(function(v: any)
								if v and v.switchable then
									subagents:set({ switchable = v.switchable, off = v.off or {} })
								end
							end, function()
								native:set(not on)
								toast.show { title = ui.t("settings.engine.couldnt_save"):get(), tone = "danger" }
							end)
						end,
					},
				},
			},
			ds.Settings.Row {
				label = ui.t("settings.providers.skills_dollar.label"),
				description = ui.t("settings.providers.skills_dollar.description"),
				control = ds.Switch { value = dollar, label = ui.t("settings.providers.skills_dollar.label"), on_change = completion("dollar") },
			},
			ds.Settings.Row {
				label = ui.t("settings.providers.skills_separate.label"),
				description = ui.t("settings.providers.skills_separate.description"),
				control = ds.Switch { value = separate, label = ui.t("settings.providers.skills_separate.label"), on_change = completion("separate") },
			},
			ui.show(function()
				return not own()
			end, ds.Settings.Row {
				label = ui.t("settings.providers.updates"),
				description = function()
					local note = POLICY_NOTE[policy:get()]
					return if note then ui.t(note):get() else ""
				end,
				control = ds.Select {
					value = policy,
					name = ui.t("settings.providers.update_policy.label"),
					size = "sm",
					w = 148,
					options = POLICIES,
					on_change = function(v: string)
						local a = agent()
						if a then
							host.call("agents.set_update_policy", { id = a.id, policy = v }):after(function() end, function()
								toast.show { title = ui.t("settings.engine.couldnt_save"):get(), tone = "danger" }
							end)
						end
					end,
				},
			}),
			ui.show(from_registry, ds.Settings.Row {
				label = ui.t("settings.providers.add.env.label"),
				description = function(): string
					return ui.t("settings.providers.add.env.description", { device = device_name() }):get()
				end,
				stacked = true,
				control = ds.TextArea {
					value = env,
					name = ui.t("settings.providers.add.env.label"),
					placeholder = "OPENAI_API_KEY=sk-…",
					mono = true,
					rows = 2,
					max_rows = 6,
					w_full = true,
					error = function(): string?
						local e = env_error:get()
						return if e == "" then nil else e
					end,
				},
			}),
			ui.show(from_registry, ds.Settings.Row {
				label = function(): string
					return ui.t("settings.providers.add.remove_row", { device = device_name() }):get()
				end,
				description = ui.t("settings.providers.add.remove_row_description"),
				danger = true,
				control = ds.Button {
					ui.t("settings.providers.add.remove"),
					tone = "danger",
					size = "sm",
					on_click = remove,
				},
			}),
		},
	}
end

-- The agents' page: the cards, then the agents still to add.
local function agents_page(): Node
	return ds.Settings.Page {
		title = ui.t("settings.providers.title"),
		description = ui.t("settings.providers.description"),
		-- Agents are per device: which one's (a choice once there's more
		-- than this one).
		-- (The same choice as Plugins': screens/settings/devices.)
		trail = devices_page.picker(shown_device, choose_device),
		-- (At the top right, on the title's line, as Plugins' is.)
		trail_top = true,
		ui.presence(kind, body, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
	}
end

-- The page in whichever view it is in.
local function page_for(v: string): Node
	if v == "add" then
		return add_agent.registry_page(nav)
	elseif v == "own" then
		return add_agent.own_page(nav)
	end
	local id = string.match(v, "^agent:(.+)$")
	if id then
		return add_agent.agent_page(id, nav)
	end
	return agents_page()
end

function M.view(): Node
	-- (Once a visit: a hot reload doesn't ask again.) The page opens on the
	-- agents; the registry is looked at too, for what it has to add.
	visits.once("providers", function()
		agents.load()
		registry.load()
		if view:peek() ~= "agents" then
			view:set("agents")
		end
	end)
	return ui.column {
		w_full = true,
		items = "center",
		-- (Each view on the page's middle, as every Settings page stands: the
		-- presence's box takes the whole width, so the page is centred in it.)
		ui.presence(function(): string
			return view:get()
		end, function(v: string): Node
			return ui.column { w_full = true, items = "center", page_for(v) }
		end, { w_full = true, enter = { opacity = { 0, 1 }, y = { 6, 0 }, duration = 160 }, exit = { opacity = 0, duration = 80 }, mode = "wait" }),
		settings_dialog(),
		resets_dialog(),
		-- (Done: the failed sign-in goes, and the card looks again.)
		sign_in_terminal.Dialog(function(id: string)
			agents.dismiss(id)
			agents.load()
		end),
	}
end

return M

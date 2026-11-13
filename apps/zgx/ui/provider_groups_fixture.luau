-- Settings → Providers with agents that keep a login per model provider,
-- mounted on its own with agents made up here (no engine in a test host):
-- what tests/provider_groups.test.luau drives. Pi has two ChatGPT accounts
-- and a Claude one (signed in by Pi's own /login: Zeron can't add those)
-- and no Copilot yet; Hermes lists its own pool, read-only; Claude Code
-- keeps one login, as ever. The groups are published as the host words
-- them (src/agents.rs `provider_groups`), and account/agents' calls are
-- answered as it would: `sign_in_to` starts that provider's sign-in,
-- `switch` moves the one in use within its provider's group alone. What
-- was asked, in order, is `groups_calls`. Here, beside app.luau, as a
-- mounted script loads modules from its own directory down.
--
--   groups.copilot_lands   the Copilot sign-in under way lands: its account

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
-- Addresses plain to start: the tests read them.
ds.PrivateText.hidden:set(false)
-- Settings' host calls (the page's settings dialog, its toasts).
require("./settings_stub")
local agents = require("@account/agents")
local registry = require("@account/registry") :: any
registry.load = function() end
local providers = require("@app/screens/settings/providers")

local function account(id: string, email: string, plan: string, provider: string?, active: boolean, switchable: boolean?): any
	return {
		id = id,
		email = email,
		plan = plan,
		session = 0.2,
		week = 0.4,
		active = active,
		switchable = switchable ~= false,
		provider = provider,
	}
end

local function agent(id: string, name: string, mark: string, accounts: { any }): any
	return {
		id = id,
		name = name,
		mark = mark,
		installed = true,
		can_install = true,
		enabled = true,
		version = "1.0.0",
		installing = false,
		login = "idle",
		accounts = accounts,
	}
end

-- Each per-provider agent's providers, in the host's order: Zeron's own
-- sign-ins first (`add`, as the host words it with and without accounts),
-- then any other its accounts are on (`hint`: how the agent adds one).
type Provider = { id: string, name: string, add: { string }?, hint: string? }
local PROVIDERS: { [string]: { Provider } } = {
	pi = {
		{ id = "openai-codex", name = "ChatGPT", add = { "Connect ChatGPT", "Add ChatGPT account" } },
		{ id = "github-copilot", name = "GitHub Copilot", add = { "Connect GitHub Copilot", "Add GitHub Copilot account" } },
		{ id = "anthropic", name = "Claude", hint = "To add another Claude account, run /login in Pi." },
	},
	hermes = {
		{ id = "openai-codex", name = "ChatGPT", add = { "Connect ChatGPT", "Add ChatGPT account" } },
		{ id = "nous", name = "Nous Portal", add = { "Connect Nous Portal", "Add Nous Portal account" } },
		{ id = "xai-oauth", name = "xAI", add = { "Connect xAI", "Add xAI account" } },
		{ id = "openrouter", name = "OpenRouter", hint = "To add another OpenRouter account, run `hermes auth add openrouter`." },
	},
}

local model: { any } = {
	agent("claude-code", "Claude Code", "claude", {
		account("cl1", "bo@example.com", "Max", nil, true),
	}),
	agent("pi", "Pi", "pi", {
		account("o1", "ada@chatgpt.test", "ChatGPT Plus", "openai-codex", true),
		account("c1", "ada@claude.test", "Claude Max", "anthropic", true),
		account("o2", "work@chatgpt.test", "ChatGPT Pro", "openai-codex", false),
	}),
	agent("hermes", "Hermes", "hermes", {
		account("h1", "first@chatgpt.test", "ChatGPT Pro", "openai-codex", true, false),
		account("h2", "second@chatgpt.test", "ChatGPT Plus", "openai-codex", false, false),
		account("h3", "OPENROUTER_API_KEY", "OpenRouter", "openrouter", true, false),
	}),
}

local function find(id: string): any
	for _, a in model do
		if a.id == id then
			return a
		end
	end
	return nil
end

-- The groups as the host publishes them (src/agents.rs).
local function groups_of(a: any, accounts: { any }): { any }?
	local list = PROVIDERS[a.id]
	if not list then
		return nil
	end
	local out = {}
	for _, p in list do
		local mine = {}
		for _, acc in accounts do
			if acc.provider == p.id then
				table.insert(mine, acc)
			end
		end
		if #mine > 0 or p.add then
			table.insert(out, {
				id = p.id,
				name = p.name,
				accounts = mine,
				add = if p.add then (if #mine == 0 then p.add[1] else p.add[2]) else nil,
				hint = if #mine > 0 then p.hint else nil,
				read_only = a.id == "hermes",
			})
		end
	end
	return out
end

local function publish()
	local list = {}
	for _, a in model do
		local item = table.clone(a)
		local accounts = {}
		for _, acc in a.accounts do
			table.insert(accounts, table.clone(acc))
		end
		item.accounts = accounts
		item.groups = groups_of(a, accounts)
		table.insert(list, item)
	end
	agents.now:set({ state = "ready", list = list } :: any)
end

local said: { string } = {}
local calls = ui.state("groups_calls", "")
local function note(what: string)
	table.insert(said, what)
	calls:set(table.concat(said, ","))
end

local stub = agents :: any
stub.load = function() end
stub.target = function() end
stub.sign_in = function(id: string)
	note(`sign_in:{id}`)
	local a = find(id)
	a.login = "waiting"
	a.login_for = if PROVIDERS[id] then PROVIDERS[id][1].id else nil
	publish()
end
stub.sign_in_to = function(id: string, provider: string)
	note(`sign_in_to:{id}:{provider}`)
	local a = find(id)
	a.login = "waiting"
	a.login_for = provider
	publish()
end
stub.cancel = function(id: string)
	note(`cancel:{id}`)
	local a = find(id)
	a.login = "idle"
	a.login_for = nil
	publish()
end
stub.dismiss = function(id: string)
	local a = find(id)
	a.login = "idle"
	a.login_for = nil
	publish()
end
stub.switch = function(id: string, account_id: string)
	note(`switch:{id}:{account_id}`)
	local a = find(id)
	local provider: string? = nil
	for _, acc in a.accounts do
		if acc.id == account_id then
			provider = acc.provider
		end
	end
	for _, acc in a.accounts do
		if acc.provider == provider then
			acc.active = acc.id == account_id
		end
	end
	publish()
end
stub.remove = function(id: string, account_id: string)
	note(`remove:{id}:{account_id}`)
end
stub.set_enabled = function() end

ui.command {
	id = "groups.copilot_lands",
	title = "The Copilot sign-in lands",
	run = function()
		local pi = find("pi")
		table.insert(pi.accounts, account("g1", "octo", "GitHub Copilot", "github-copilot", true))
		pi.login = "idle"
		pi.login_for = nil
		publish()
	end,
}

publish()

return ui.scroll {
	id = "page",
	w_full = true,
	h_full = true,
	bg = "$zeron.window",
	ui.column { w_full = true, px = "$space.6", py = "$space.6", providers.view() },
}

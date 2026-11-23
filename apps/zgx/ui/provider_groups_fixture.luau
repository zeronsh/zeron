-- Settings → Providers with agents that keep a login per model provider,
-- mounted on its own with agents made up here (no engine in a test host):
-- what tests/provider_groups.test.luau drives. Pi has two ChatGPT accounts
-- and a Claude one (signed in by Pi's own /login: Zeron can't add those)
-- and no Copilot yet; Hermes lists its own pool, read-only; Claude Code
-- keeps one login, as ever. The groups are published as the host words
-- them (src/agents.rs `provider_groups`), and account/agents' calls are
-- answered as it would: `sign_in_to` starts that provider's sign-in,
-- `switch` moves the one in use within its provider's group alone,
-- `add_key` takes a key starting "sk-good" and refuses any other. What
-- was asked, in order, is `groups_calls`. Here, beside app.luau, as a
-- mounted script loads modules from its own directory down.
--
--   groups.copilot_lands   the Copilot sign-in under way lands: its account
--   groups.remote          the page shows another device's agents

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

-- An API key's row: no plan, no windows.
local function key_account(a: any): any
	a.key = true
	a.session = 0
	a.week = 0
	return a
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

-- Each per-provider agent's providers, in the host's order (src/agents.rs
-- `provider_groups`): who Zeron signs you in to for each (`login`), who
-- issues the key it adds (`key`), and how the agent adds one where Zeron
-- can do neither (`hint`).
type Provider = { id: string, name: string, login: string?, key: string?, hint: string? }
local PROVIDERS: { [string]: { Provider } } = {
	pi = {
		{ id = "openai-codex", name = "ChatGPT", login = "ChatGPT" },
		{ id = "anthropic", name = "Anthropic", login = "Claude", key = "Anthropic" },
		{ id = "github-copilot", name = "GitHub Copilot", login = "GitHub Copilot" },
		{ id = "openai", name = "OpenAI", key = "OpenAI" },
		{ id = "openrouter", name = "OpenRouter", key = "OpenRouter" },
		{ id = "google", name = "Google Gemini", key = "Google Gemini" },
		{ id = "xai", name = "xAI", key = "xAI" },
		{ id = "deepseek", name = "DeepSeek", key = "DeepSeek" },
		{ id = "kimi-coding", name = "Kimi" },
	},
	hermes = {
		{ id = "openai-codex", name = "ChatGPT", login = "ChatGPT" },
		{ id = "nous", name = "Nous Portal", login = "Nous Portal" },
		{ id = "xai-oauth", name = "xAI", login = "xAI" },
		{ id = "anthropic", name = "Anthropic", key = "Anthropic" },
		{ id = "openai-api", name = "OpenAI", key = "OpenAI" },
		{ id = "openrouter", name = "OpenRouter", key = "OpenRouter" },
		{ id = "gemini", name = "Google Gemini", key = "Google Gemini" },
		{ id = "xai", name = "xAI", key = "xAI" },
		{ id = "deepseek", name = "DeepSeek", key = "DeepSeek" },
	},
}

-- (Who each provider is, as the host names it in a row.)
local VENDOR_NAME = { anthropic = "Anthropic", openai = "OpenAI", ["openai-api"] = "OpenAI", openrouter = "OpenRouter", google = "Google Gemini", gemini = "Google Gemini", xai = "xAI", deepseek = "DeepSeek" }

local model: { any } = {
	agent("claude-code", "Claude Code", "claude", {
		account("cl1", "bo@example.com", "Max", nil, true),
	}),
	agent("codex", "Codex", "openai", {
		account("cx1", "ada@example.com", "Pro", nil, true),
		account("cx2", "work@example.com", "Plus", nil, false),
	}),
	agent("pi", "Pi", "pi", {
		account("o1", "ada@chatgpt.test", "ChatGPT Plus", "openai-codex", true),
		account("c1", "ada@claude.test", "Claude Max", "anthropic", true),
		account("o2", "work@chatgpt.test", "ChatGPT Pro", "openai-codex", false),
		account("o3", "old@chatgpt.test", "ChatGPT Plus", "openai-codex", false),
		account("k9", "kimi@example.test", "Kimi", "kimi-coding", true),
	}),
	agent("hermes", "Hermes", "hermes", {
		account("h1", "first@chatgpt.test", "ChatGPT Pro", "openai-codex", true, false),
		account("h2", "second@chatgpt.test", "ChatGPT Plus", "openai-codex", false, false),
		-- (Seeded from OPENROUTER_API_KEY: never titled by that variable.)
		key_account(account("h3", "API key ···9f3e", "OpenRouter · from environment", "openrouter", true, false)),
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
		if #mine > 0 or p.login or p.key then
			table.insert(out, {
				id = p.id,
				name = p.name,
				accounts = mine,
				login = p.login,
				key = p.key,
				hint = if #mine > 0 then p.hint else nil,
				read_only = a.id == "hermes",
			})
		end
	end
	return out
end

-- Whose agents show: this Mac's (nil), or another device's (`groups.remote`).
local target = ui.state("groups_target", nil :: string?)

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
	agents.now:set({ state = "ready", list = list, target = target:peek() } :: any)
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
-- A switch: o3's is refused, in the engine's words; any other taken.
stub.switch = function(id: string, account_id: string): any
	note(`switch:{id}:{account_id}`)
	if account_id == "o3" then
		return {
			after = function(_: any, _done: (any) -> (), failed: (any) -> ())
				failed({ message = "That saved login no longer exists — refresh and try again." })
			end,
		}
	end
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
	return { after = function(_: any, done: (any) -> ()) done(nil) end }
end
-- A key's name: an empty one its default; one holding "sk-" refused (as the
-- engine refuses a name that looks like a key).
stub.rename = function(id: string, account_id: string, name: string): any
	note(`rename:{id}:{account_id}:{name}`)
	local refused = string.find(name, "sk-", 1, true) ~= nil
	if not refused then
		for _, acc in find(id).accounts do
			if acc.id == account_id then
				local who = VENDOR_NAME[acc.provider] or acc.provider
				local tail = string.sub(acc.id, -4)
				if name == "" then
					acc.email, acc.plan, acc.named = `API key ···{tail}`, who, false
				else
					acc.email, acc.plan, acc.named = name, `{who} · API key`, true
				end
			end
		end
		publish()
	end
	return {
		after = function(_: any, done: (any) -> (), failed: (any) -> ())
			if refused then
				failed({ message = "That looks like a key — give it a name instead." })
			else
				done(nil)
			end
		end,
	}
end
stub.remove = function(id: string, account_id: string)
	note(`remove:{id}:{account_id}`)
end
stub.set_enabled = function() end
-- A key: one starting "sk-good" is taken (its row, by its last four); one
-- starting "sk-slow" is still being checked (for as long as the test
-- likes); anything else is refused in the provider's words. What was typed
-- is never noted: only that a key was added, and for which provider.
local VENDOR = { anthropic = "Anthropic", openai = "OpenAI", ["openai-api"] = "OpenAI", openrouter = "OpenRouter", google = "Google", gemini = "Google", xai = "xAI", deepseek = "DeepSeek" }
stub.add_key = function(id: string, provider: string, key: string, name: string?): any
	if string.sub(key, 1, 7) == "sk-slow" then
		note(`add_key:{id}:{provider}:checking`)
		return { after = function() end }
	end
	local ok = string.sub(key, 1, 7) == "sk-good"
	note(`add_key:{id}:{provider}:{if ok then "ok" else "refused"}`)
	if ok then
		local a = find(id)
		local live = true
		for _, acc in a.accounts do
			if acc.provider == provider and acc.active then
				live = false
			end
		end
		local who = VENDOR_NAME[provider] or provider
		local tail = string.sub(key, -4)
		-- As the host words a key: its name over "<Provider> · API key", or
		-- "API key ···<last4>" over its provider.
		local row = key_account(account(`key-{provider}-{tail}`, name or `API key ···{tail}`, if name then `{who} · API key` else who, provider, live))
		row.named = name ~= nil
		table.insert(a.accounts, row)
		publish()
	end
	return {
		after = function(_: any, done: (any) -> (), failed: (any) -> ())
			if ok then
				done(nil)
			else
				failed({ message = `{VENDOR[provider] or provider} didn't accept that key — check it and try again.` })
			end
		end,
	}
end

ui.command {
	id = "groups.copilot_lands",
	title = "The Copilot sign-in lands",
	run = function()
		local pi = find("pi")
		for _, acc in pi.accounts do
			if acc.id == "g1" then
				return
			end
		end
		table.insert(pi.accounts, account("g1", "octo", "GitHub Copilot", "github-copilot", true))
		pi.login = "idle"
		pi.login_for = nil
		publish()
	end,
}

ui.command {
	id = "groups.clear_keys",
	title = "Forget the keys added here",
	run = function()
		for _, a in model do
			local kept = {}
			for _, acc in a.accounts do
				if not (acc.key and string.sub(acc.id, 1, 4) == "key-") then
					table.insert(kept, acc)
				end
			end
			a.accounts = kept
		end
		publish()
	end,
}
ui.command {
	id = "groups.remote",
	title = "Show another device's agents",
	run = function()
		target:set("box")
		publish()
	end,
}

publish()

return ui.column {
	w_full = true,
	h_full = true,
	ds.toast.Host {},
	ui.scroll {
		id = "page",
		w_full = true,
		h_full = true,
		bg = "$zeron.window",
		ui.column { w_full = true, px = "$space.6", py = "$space.6", providers.view() },
	},
}

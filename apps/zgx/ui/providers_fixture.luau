-- Your agents, mounted on its own with agents made up here (no engine in a
-- test host): the step tests/providers_stability.test.luau drives. The
-- host's calls (account/agents) are stubbed to change this data as the
-- engine would; `ui.command`s change it as the engine would on its own (an
-- account signed in elsewhere, one removed). Here, beside app.luau, as a
-- mounted script loads modules from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
-- Addresses plain to start (hiding is the app's default): the tests read
-- them, and private_text's turn hiding on themselves.
ds.PrivateText.hidden:set(false)
local agents = require("@account/agents")
local providers = require("@screens/onboarding/providers")

type Account = { id: string, email: string, plan: string, session: number, week: number, active: boolean }

local function agent(id: string, name: string, mark: string, accounts: { Account }): any
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

local model: { any } = {
	agent("claude-code", "Claude Code", "claude", {}),
	agent("codex", "Codex", "openai", {
		{ id = "cx1", email = "ada@example.com", plan = "Pro", session = 0.2, week = 0.5, active = true },
	}),
	agent("cursor", "Cursor", "cursor", {}),
	agent("grok", "Grok", "grok", {}),
	agent("devin", "Devin", "devin", {}),
	agent("pi", "Pi", "pi", {}),
}

-- What the host would publish: each agent with its accounts and sign-in.
-- (Also by agent, `accounts` and `logins`, the shape an earlier step read,
-- so this test ran against it too.)
local function publish()
	local list, by, logins = {}, {}, {}
	for _, a in model do
		local item = table.clone(a)
		local accounts = {}
		for _, acc in a.accounts do
			table.insert(accounts, table.clone(acc))
		end
		item.accounts = accounts
		table.insert(list, item)
		by[a.id] = accounts
		if a.login ~= "idle" then
			logins[a.id] = a.login
		end
	end
	agents.now:set({ state = "ready", list = list, accounts = by, logins = logins } :: any)
end

local function find(id: string): any
	for _, a in model do
		if a.id == id then
			return a
		end
	end
	return nil
end

-- What the step asks of the engine, answered here.
local said: { string } = {}
local calls = ui.state("calls", "")
local function note(what: string)
	table.insert(said, what)
	calls:set(table.concat(said, ","))
end
local stub = agents :: any
stub.load = function() end
stub.sign_in = function(id: string)
	note(`sign_in:{id}`)
	find(id).login = "waiting"
	publish()
end
stub.cancel = function(id: string)
	note(`cancel:{id}`)
	find(id).login = "idle"
	publish()
end
stub.dismiss = function(id: string)
	find(id).login = "idle"
	publish()
end
stub.set_enabled = function(id: string, on: boolean)
	note(`set_enabled:{id}:{tostring(on)}`)
	find(id).enabled = on
	publish()
end
stub.switch = function() end
stub.remove = function() end
stub.install = function() end

-- What changes on the engine's side: an account signed in elsewhere, its
-- usage moving, one removed.
ui.command {
	id = "add_account",
	title = "Add an account to Grok",
	run = function()
		table.insert(find("grok").accounts, { id = "gk1", email = "bo@example.com", plan = "SuperGrok", session = 0.1, week = 0.3, active = true })
		publish()
	end,
}
ui.command {
	id = "usage",
	title = "Move Codex's usage",
	run = function()
		find("codex").accounts[1].session = 0.9
		find("codex").version = "1.0.1"
		publish()
	end,
}
ui.command {
	id = "remove_account",
	title = "Remove Grok's account",
	run = function()
		find("grok").accounts = {}
		publish()
	end,
}

publish()

return ds.Onboarding.Frame {
	providers {
		step = 3,
		count = 3,
		on_back = function() end,
		on_continue = function() end,
	},
}

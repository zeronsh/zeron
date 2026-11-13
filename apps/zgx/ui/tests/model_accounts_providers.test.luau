-- The model picker's Account row for an agent that keeps a login per model
-- provider (Pi, OpenCode, Hermes): it is the chosen model's provider's
-- account — "ChatGPT account" for a ChatGPT model, "Claude account" for a
-- Claude one — listing and switching that provider's accounts alone, so
-- switching ChatGPT accounts never touches the Claude one. A provider with
-- no account yet that Zeron can sign in to says None and offers to add one;
-- one Zeron can't add to offers nothing more; a provider with neither has
-- no row. On ui/chat_fixture.luau, its composer reading `agents.on_device`
-- and `agents.logins` as the host publishes them (src/agents.rs).

local function window(label: string, used: number): any
	return { label = label, used = used }
end

local EFFORT = { { value = "low", label = "Low" }, { value = "high", label = "High" } }
local function provider(id: string, name: string): any
	return { id = id, name = name }
end

local CATALOG = {
	state = "ready",
	harnesses = { { id = "claude-code", label = "Claude", mark = "claude" }, { id = "pi", label = "Pi", mark = "pi" } },
	models = {
		{ value = "opus", label = "Opus", harness = "claude-code", efforts = EFFORT, effort = "high" },
		{ value = "pi/openai-codex/gpt-6", label = "GPT-6", harness = "pi", provider = provider("openai-codex", "Codex") },
		{ value = "pi/anthropic/opus-5", label = "Opus 5", harness = "pi", provider = provider("anthropic", "Anthropic") },
		{ value = "pi/github-copilot/gpt-6", label = "GPT-6", harness = "pi", provider = provider("github-copilot", "Copilot") },
		{ value = "pi/openrouter/kimi", label = "Kimi", harness = "pi", provider = provider("openrouter", "OpenRouter") },
	},
}

local function pi_accounts(): { any }
	return {
		{ id = "o1", email = "ada@chatgpt.test", plan = "ChatGPT Plus", active = true, can_switch = true, provider = "openai-codex", group = "ChatGPT", windows = { window("5h", 0.3) } },
		{ id = "c1", email = "ada@claude.test", plan = "Claude Max", active = true, can_switch = true, provider = "anthropic", group = "Claude", windows = { window("Week", 0.6) } },
		{ id = "o2", email = "work@chatgpt.test", plan = "ChatGPT Pro", active = false, can_switch = true, provider = "openai-codex", group = "ChatGPT", windows = { window("5h", 0.05) } },
	}
end

local LOGINS = {
	pi = {
		["openai-codex"] = { name = "ChatGPT", add = "Add ChatGPT account" },
		["github-copilot"] = { name = "GitHub Copilot", add = "Add GitHub Copilot account" },
	},
}

local function with_model(model: string): any
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:frames(30)
	app:set_state("private_text_hidden", false)
	app:set_state("chat.catalog", CATALOG)
	app:set_state("agents.logins", LOGINS)
	app:set_state("agents.on_device", { [""] = { pi = pi_accounts() } })
	app:set_state("chat_model", model)
	app:frames(10)
	return app
end

local function open_panel(app: any)
	app:click('role:button "Model"')
	app:frames(30)
end

test("a ChatGPT model's row is its ChatGPT account, and lists ChatGPT's alone", function()
	local app = with_model("pi/openai-codex/gpt-6")
	expect(app:has("#model-picker-usage")).to_be(true)
	open_panel(app)
	local row = app:tree("#model-picker-account")
	expect(row).to_contain("ChatGPT account")
	expect(row).to_contain("ada@chatgpt.test")
	expect(row).to_contain("30%")
	app:click("#model-picker-account")
	app:frames(30)
	local all = app:tree()
	expect(all).to_contain("work@chatgpt.test")
	expect(string.find(all, "ada@claude.test", 1, true)).to_be(nil)
	-- Another ChatGPT account, added from here.
	expect(app:has('"Add ChatGPT account"')).to_be(true)
	app:click('~"work@chatgpt.test"')
	app:frames(30)
	expect(app:state("calls")).to_contain("switch_on::pi:o2")
end)

test("a Claude model's row is its Claude account; switching ChatGPT never touched it", function()
	local app = with_model("pi/anthropic/opus-5")
	open_panel(app)
	local row = app:tree("#model-picker-account")
	expect(row).to_contain("Claude account")
	expect(row).to_contain("ada@claude.test")
	expect(row).to_contain("60%")
	app:click("#model-picker-account")
	app:frames(30)
	local all = app:tree()
	expect(string.find(all, "work@chatgpt.test", 1, true)).to_be(nil)
	-- Pi's own /login adds Claude accounts: nothing to add from here.
	expect(string.find(all, "Add Claude account", 1, true)).to_be(nil)
end)

test("a provider with no account yet says None and offers to add one", function()
	local app = with_model("pi/github-copilot/gpt-6")
	-- (Nothing in use: no ring.)
	expect(app:has("#model-picker-usage")).to_be(false)
	open_panel(app)
	local row = app:tree("#model-picker-account")
	expect(row).to_contain("GitHub Copilot account")
	expect(row).to_contain("None")
	app:click("#model-picker-account")
	app:frames(30)
	app:click('"Add GitHub Copilot account"')
	app:frames(30)
	local calls = app:state("calls")
	expect(calls).to_contain("target:")
	expect(calls).to_contain("sign_in_to:pi:github-copilot")
end)

test("a provider with no account and no sign-in of Zeron's has no row", function()
	local app = with_model("pi/openrouter/kimi")
	expect(app:has("#model-picker-usage")).to_be(false)
	open_panel(app)
	expect(app:has("#model-picker-account")).to_be(false)
end)

test("an agent with one login keeps its plain Account row", function()
	local app = with_model("opus")
	app:set_state("agents.on_device", {
		[""] = {
			["claude-code"] = {
				{ id = "a1", email = "wing@example.test", plan = "Max", active = true, can_switch = true, windows = { window("5h", 0.34) } },
			},
		},
	})
	app:frames(10)
	open_panel(app)
	local row = app:tree("#model-picker-account")
	expect(row).to_contain("Account")
	expect(string.find(row, "ChatGPT", 1, true)).to_be(nil)
	expect(row).to_contain("wing@example.test")
end)

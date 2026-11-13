-- Settings → Providers for an agent that keeps a login per model provider
-- (Pi, OpenCode, Hermes): its accounts under a heading per provider, each
-- group with its own one in use and its own "Add <provider> account"; a
-- provider with none yet is a Connect row at the card's foot; one Zeron
-- can't sign in to says how the agent does. A sign-in shows in its own
-- provider's row, a switch moves the one in use within its group alone,
-- Hermes' own pool has no actions, and an agent with one login looks as it
-- always has. On ui/provider_groups_fixture.luau.

local PAGE = "#page"
local PI_CHATGPT = "#account-group-pi-openai-codex"
local PI_CLAUDE = "#account-group-pi-anthropic"
local PI_COPILOT = "#account-group-pi-github-copilot"
local PI_CONNECT_COPILOT = "#account-connect-pi-github-copilot"

local function settle(app: any)
	app:frames(30)
end

local function open(): any
	local app = mount("provider_groups_fixture.luau")
	app:resize(900, 2000)
	settle(app)
	return app
end

-- How many times `text` shows under `scope`.
local function count(app: any, scope: string, text: string): number
	local _, n = string.gsub(app:tree(scope), text:gsub("%p", "%%%0"), "")
	return n
end

local function shows(app: any, scope: string, text: string): boolean
	return count(app, scope, text) > 0
end

test("a per-provider agent's accounts are grouped by provider, each with its own in use and add row", function()
	local app = open()
	-- ChatGPT: its heading, both accounts, one in use, its own add row.
	expect(shows(app, PI_CHATGPT, "ChatGPT")).to_be(true)
	expect(shows(app, PI_CHATGPT, "ada@chatgpt.test")).to_be(true)
	expect(shows(app, PI_CHATGPT, "work@chatgpt.test")).to_be(true)
	expect(shows(app, PI_CHATGPT, "ada@claude.test")).to_be(false)
	expect(count(app, PI_CHATGPT, "In use")).to_be(1)
	expect(app:has(PI_CHATGPT .. ' >> role:button "Add ChatGPT account"')).to_be(true)
	-- Claude: in use in its own group too, and no add row: Pi's /login adds
	-- them, and it says so.
	expect(shows(app, PI_CLAUDE, "Claude")).to_be(true)
	expect(count(app, PI_CLAUDE, "In use")).to_be(1)
	expect(shows(app, PI_CLAUDE, "To add another Claude account, run /login in Pi.")).to_be(true)
	expect(shows(app, PI_CLAUDE, "Add ")).to_be(false)
	-- No Copilot account yet: a Connect row at the foot, no heading.
	expect(app:has(PI_COPILOT)).to_be(false)
	expect(app:has(PI_CONNECT_COPILOT .. ' >> role:button "Connect GitHub Copilot"')).to_be(true)
	-- (No card-wide "Add account" for it.)
	expect(app:has('#account-group-pi-openai-codex >> role:button "Add account"')).to_be(false)
end)

test("an add row signs in to its own provider, the sign-in in that row alone", function()
	local app = open()
	app:click(PI_CHATGPT .. ' >> role:button "Add ChatGPT account"')
	settle(app)
	expect(app:state("groups_calls")).to_contain("sign_in_to:pi:openai-codex")
	expect(shows(app, PI_CHATGPT, "Signing in to ChatGPT")).to_be(true)
	-- The other providers' rows stay as they were.
	expect(app:has(PI_CONNECT_COPILOT .. ' >> role:button "Connect GitHub Copilot"')).to_be(true)
	expect(shows(app, PI_CLAUDE, "Signing in")).to_be(false)
	app:click(PI_CHATGPT .. ' >> role:button "Cancel"')
	settle(app)
	expect(app:state("groups_calls")).to_contain("cancel:pi")
	expect(app:has(PI_CHATGPT .. ' >> role:button "Add ChatGPT account"')).to_be(true)
end)

test("connecting a provider's first account brings its group, with its own add row", function()
	local app = open()
	app:click(PI_CONNECT_COPILOT .. ' >> role:button "Connect GitHub Copilot"')
	settle(app)
	expect(app:state("groups_calls")).to_contain("sign_in_to:pi:github-copilot")
	expect(shows(app, PI_CONNECT_COPILOT, "Signing in to GitHub Copilot")).to_be(true)
	-- (ChatGPT's own add row is untouched.)
	expect(app:has(PI_CHATGPT .. ' >> role:button "Add ChatGPT account"')).to_be(true)
	app:command("groups.copilot_lands")
	settle(app)
	expect(app:has(PI_CONNECT_COPILOT)).to_be(false)
	expect(shows(app, PI_COPILOT, "GitHub Copilot")).to_be(true)
	expect(shows(app, PI_COPILOT, "octo")).to_be(true)
	expect(count(app, PI_COPILOT, "In use")).to_be(1)
	expect(app:has(PI_COPILOT .. ' >> role:button "Add GitHub Copilot account"')).to_be(true)
	-- Each group still has exactly its own one in use.
	expect(count(app, PI_CHATGPT, "In use")).to_be(1)
	expect(count(app, PI_CLAUDE, "In use")).to_be(1)
end)

test("switching a ChatGPT account moves the one in use within ChatGPT alone", function()
	local app = open()
	app:click('#account-row-o2 >> role:button "Account actions"')
	settle(app)
	app:click('"Switch to this account"')
	settle(app)
	expect(app:state("groups_calls")).to_contain("switch:pi:o2")
	expect(count(app, PI_CHATGPT, "In use")).to_be(1)
	expect(count(app, PI_CLAUDE, "In use")).to_be(1)
	expect(shows(app, PI_CLAUDE, "ada@claude.test")).to_be(true)
end)

test("Hermes' own pool is grouped and read-only, adding through Hermes", function()
	local app = open()
	local chatgpt = "#account-group-hermes-openai-codex"
	expect(shows(app, chatgpt, "first@chatgpt.test")).to_be(true)
	expect(count(app, chatgpt, "In use")).to_be(1)
	-- No actions on Hermes' accounts: Hermes chooses among them itself.
	expect(app:has('#account-row-h1 >> role:button "Account actions"')).to_be(false)
	expect(app:has('#account-row-o1 >> role:button "Account actions"')).to_be(true)
	expect(app:has(chatgpt .. ' >> role:button "Add ChatGPT account"')).to_be(true)
	expect(shows(app, "#account-group-hermes-openrouter", "To add another OpenRouter account, run")).to_be(true)
	expect(app:has('#account-connect-hermes-xai-oauth >> role:button "Connect xAI"')).to_be(true)
	app:click('#account-connect-hermes-nous >> role:button "Connect Nous Portal"')
	settle(app)
	expect(app:state("groups_calls")).to_contain("sign_in_to:hermes:nous")
end)

test("an agent with one login looks as it always has", function()
	local app = open()
	expect(app:has('#account-group-claude-code-')).to_be(false)
	expect(shows(app, PAGE, "bo@example.com")).to_be(true)
	app:click('role:button "Add account"')
	settle(app)
	expect(app:state("groups_calls")).to_contain("sign_in:claude-code")
end)

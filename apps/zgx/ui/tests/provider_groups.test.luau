-- Settings → Providers for an agent that keeps a login per model provider
-- (Pi, OpenCode, Hermes): the same card as Claude Code's and Codex's — a
-- row per account (a provider's together, its plan line saying whose), the
-- one in use marked per provider, and every add row at the foot ("Add
-- ChatGPT account", "Add Claude account", …, "Add API key"). A sign-in or a
-- key takes its add row's place; a key is typed on this Mac only and never
-- shown again; a switch moves the one in use within its provider alone;
-- Hermes' own pool has no actions. On ui/provider_groups_fixture.luau.

local PAGE = "#page"
local KEY = "sk-good-ZEROTESTsecret-WXYZ"

local function settle(app: any)
	app:frames(30)
end

local function open(): any
	local app = mount("provider_groups_fixture.luau")
	app:resize(900, 2600)
	settle(app)
	return app
end

local function shows(app: any, scope: string, text: string): boolean
	return app:has(scope) and string.find(app:tree(scope), text, 1, true) ~= nil
end

-- Whether the key dialog is open (a closed one stays in the tree, hidden,
-- as it leaves: a test host's frames pass no time).
local function key_dialog_open(app: any): boolean
	local line = app:tree():match('[^\n]*dialog "Add API key"[^\n]*')
	return line ~= nil and line:find(" hidden", 1, true) == nil
end

-- A node's line in the tree (`kind "label"` under `scope`), shown (not
-- leaving, hidden): nil if there's none.
local function line_of(app: any, scope: string, kind: string, label: string): string?
	for line in (if scope == "" then app:tree() else app:tree(scope)):gmatch("[^\n]+") do
		if line:find(` {kind} "{label}"`, 1, true) and not line:find(" hidden", 1, true) then
			return line
		end
	end
	return nil
end

local function in_use(app: any, row: string): boolean
	return shows(app, `#account-row-{row}`, "In use")
end

-- The rows' ids under `scope`, top to bottom (`#account-row-<id>`,
-- `#account-add-…`).
local function order(app: any, scope: string): { string }
	local out = {}
	for line in app:tree(scope):gmatch("[^\n]+") do
		local id = line:match("#account%-row%-([%w%-]+)") or line:match("#(account%-add%-[%w%-]+)")
		if id then
			table.insert(out, id)
		end
	end
	return out
end

test("a per-provider agent's card is Claude Code's card: its accounts, a provider's together, then every add row", function()
	local app = open()
	local pi = order(app, PAGE)
	-- (From Pi's first row on.)
	local from = table.find(pi, "o1") :: number
	local rows = table.move(pi, from, from + 10, 1, {})
	expect(table.concat(rows, ",")).to_be(
		"o1,o2,o3,c1,k9,account-add-pi-openai-codex,account-add-pi-anthropic,account-add-pi-github-copilot,account-add-key-pi,h1,h2"
	)
	-- Whose each is, on its plan line, as any account's plan.
	expect(shows(app, "#account-row-o1", "ChatGPT Plus")).to_be(true)
	expect(shows(app, "#account-row-c1", "Claude Max")).to_be(true)
	-- One in use per provider.
	expect(in_use(app, "o1")).to_be(true)
	expect(in_use(app, "o2")).to_be(false)
	expect(in_use(app, "c1")).to_be(true)
	expect(app:has('#account-add-pi-openai-codex >> role:button "Add ChatGPT account"')).to_be(true)
	expect(app:has('#account-add-pi-anthropic >> role:button "Add Claude account"')).to_be(true)
	expect(app:has('#account-add-key-pi >> role:button "Add API key"')).to_be(true)
	-- No section chrome, no CLI hints.
	expect(string.find(app:tree(PAGE), "2 accounts", 1, true)).to_be(nil)
	expect(app:has("#account-group-pi-openai-codex")).to_be(false)
	expect(string.find(app:tree(PAGE), "/login", 1, true)).to_be(nil)
end)

test("an add row's sign-in takes its place, the others standing", function()
	local app = open()
	app:click('#account-add-pi-openai-codex >> role:button "Add ChatGPT account"')
	settle(app)
	expect(app:state("groups_calls")).to_contain("sign_in_to:pi:openai-codex")
	expect(shows(app, "#account-add-pi-openai-codex", "Signing in to ChatGPT")).to_be(true)
	expect(app:has('#account-add-pi-anthropic >> role:button "Add Claude account"')).to_be(true)
	app:click('#account-add-pi-openai-codex >> role:button "Cancel"')
	settle(app)
	expect(app:state("groups_calls")).to_contain("cancel:pi")
	expect(app:has('#account-add-pi-openai-codex >> role:button "Add ChatGPT account"')).to_be(true)
end)

test("a provider's first account joins the list, in use for its provider alone", function()
	local app = open()
	app:click('#account-add-pi-github-copilot >> role:button "Add GitHub Copilot account"')
	settle(app)
	expect(app:state("groups_calls")).to_contain("sign_in_to:pi:github-copilot")
	expect(shows(app, "#account-add-pi-github-copilot", "Signing in to GitHub Copilot")).to_be(true)
	app:command("groups.copilot_lands")
	settle(app)
	expect(shows(app, "#account-row-g1", "octo")).to_be(true)
	expect(in_use(app, "g1")).to_be(true)
	expect(in_use(app, "o1")).to_be(true)
	expect(in_use(app, "c1")).to_be(true)
	expect(app:has('#account-add-pi-github-copilot >> role:button "Add GitHub Copilot account"')).to_be(true)
end)

test("Add API key opens a dialog: a provider select, the key; Save waits for a key; a refused key says why and stays", function()
	local app = open()
	app:click('#account-add-key-pi >> role:button "Add API key"')
	settle(app)
	local dialog = "#key-dialog-pi"
	expect(key_dialog_open(app)).to_be(true)
	-- The first provider Pi has no key for yet, chosen.
	expect(shows(app, dialog .. ' >> role:button "Provider"', "Anthropic")).to_be(true)
	expect(shows(app, 'role:dialog "Add API key"', "Pi uses this key for Anthropic models.")).to_be(true)
	-- Nothing typed: Save waits.
	expect((line_of(app, 'role:dialog "Add API key"', "button", "Save") or ""):find("disabled", 1, true) ~= nil).to_be(true)
	-- Every provider Pi takes a key for, in the select.
	app:click(dialog .. ' >> role:button "Provider"')
	settle(app)
	for _, name in { "Anthropic", "OpenAI", "OpenRouter", "Google Gemini", "xAI", "DeepSeek" } do
		expect(app:has(`role:menuitem "{name}"`)).to_be(true)
	end
	app:key("Escape")
	settle(app)
	local field = dialog .. ' >> role:textbox "API key"'
	app:type(field, "sk-bad-0000")
	app:click('role:button "Save"')
	settle(app)
	expect(app:state("groups_calls")).to_contain("add_key:pi:anthropic:refused")
	expect(key_dialog_open(app)).to_be(true)
	expect(shows(app, dialog, "Anthropic didn't accept that key")).to_be(true)
end)

test("while a key is checked, Save spins and the inputs hold still", function()
	local app = open()
	app:click('#account-add-key-pi >> role:button "Add API key"')
	settle(app)
	app:type('#key-dialog-pi >> role:textbox "API key"', "sk-slow-0000")
	app:key("Enter")
	settle(app)
	expect(app:state("groups_calls")).to_contain("add_key:pi:anthropic:checking")
	expect(key_dialog_open(app)).to_be(true)
	-- (The provider holds still while its key is checked.)
	expect((line_of(app, "#key-dialog-pi", "button", "Provider") or ""):find("disabled", 1, true) ~= nil).to_be(true)
	-- Nothing closes it meanwhile: the key it's checking is kept if it
	-- passes. Cancel waits; Escape too.
	expect((line_of(app, 'role:dialog "Add API key"', "button", "Cancel") or ""):find("disabled", 1, true) ~= nil).to_be(true)
	app:key("Escape")
	settle(app)
	expect(key_dialog_open(app)).to_be(true)
end)

test("a key that's taken closes the dialog and lists by its default name, the key itself nowhere", function()
	local app = open()
	app:click('#account-add-key-pi >> role:button "Add API key"')
	settle(app)
	app:click('#key-dialog-pi >> role:button "Provider"')
	settle(app)
	app:click('role:menuitem "OpenRouter"')
	settle(app)
	app:type('#key-dialog-pi >> role:textbox "API key"', KEY)
	app:key("Enter")
	app:frames(60)
	expect(app:state("groups_calls")).to_contain("add_key:pi:openrouter:ok")
	expect(key_dialog_open(app)).to_be(false)
	local row = "#account-row-key-openrouter-WXYZ"
	-- Unnamed: "API key" and its last four, over its provider.
	expect(shows(app, row, "API key ···WXYZ")).to_be(true)
	expect(shows(app, row, "OpenRouter")).to_be(true)
	expect(shows(app, row, "OpenRouter · API key")).to_be(false)
	-- Its face is its provider's mark, not a person's: logins keep theirs.
	expect(shows(app, row, '"Account"')).to_be(false)
	expect(shows(app, "#account-row-o1", '"Account"')).to_be(true)
	expect(in_use(app, "key-openrouter-WXYZ")).to_be(true)
	-- (A key has no plan: no meters on its row.)
	expect(shows(app, row, "Session")).to_be(false)
	expect(string.find(app:tree(), "ZEROTESTsecret", 1, true)).to_be(nil)
	expect(string.find(app:state("groups_calls"), "ZEROTEST", 1, true)).to_be(nil)
	-- Opened again: nothing typed is kept.
	app:click('#account-add-key-pi >> role:button "Add API key"')
	settle(app)
	expect(string.find(app:tree(), "ZEROTESTsecret", 1, true)).to_be(nil)
end)

test("a key named as it's added, a second of one provider its own row, and a rename with no key asked", function()
	local app = open()
	local function add(key: string, name: string?)
		app:click('#account-add-key-pi >> role:button "Add API key"')
		settle(app)
		app:click('#key-dialog-pi >> role:button "Provider"')
		settle(app)
		app:click('role:menuitem "OpenRouter"')
		settle(app)
		app:type('#key-dialog-pi >> role:textbox "API key"', key)
		if name then
			app:type('#key-dialog-pi >> role:textbox "Name (optional)"', name)
		end
		app:click('role:button "Save"')
		app:frames(60)
	end
	add("sk-good-one-AAAA", "Work")
	add("sk-good-two-BBBB", nil)
	-- Named: its name over "OpenRouter · API key", the last four nowhere.
	expect(shows(app, "#account-row-key-openrouter-AAAA", "Work")).to_be(true)
	expect(shows(app, "#account-row-key-openrouter-AAAA", "OpenRouter · API key")).to_be(true)
	expect(shows(app, "#account-row-key-openrouter-AAAA", "···AAAA")).to_be(false)
	-- Unnamed: "API key ···BBBB" over its provider.
	expect(shows(app, "#account-row-key-openrouter-BBBB", "API key ···BBBB")).to_be(true)
	-- Two keys of one provider: two rows, one in use.
	expect(in_use(app, "key-openrouter-AAAA")).to_be(true)
	expect(in_use(app, "key-openrouter-BBBB")).to_be(false)
	-- Renamed from its menu: just its name.
	app:click('#account-row-key-openrouter-BBBB >> role:button "Account actions"')
	settle(app)
	app:click('"Rename"')
	settle(app)
	local field = '#rename-dialog-pi >> role:textbox "Name"'
	expect(app:has(field)).to_be(true)
	expect(app:has('#rename-dialog-pi >> role:textbox "API key"')).to_be(false)
	-- A name holding part of a key: refused, said why, still open.
	app:key("cmd+a")
	app:type(field, "my sk-good key")
	app:click('role:button "Save"')
	settle(app)
	expect(shows(app, "#rename-dialog-pi", "looks like a key")).to_be(true)
	app:key("cmd+a")
	app:type(field, "Personal")
	app:click('role:button "Save"')
	app:frames(60)
	expect(app:state("groups_calls")).to_contain("rename:pi:key-openrouter-BBBB:Personal")
	expect(shows(app, "#account-row-key-openrouter-BBBB", "Personal")).to_be(true)
	-- (Logins aren't renamed: no Rename in their menu.)
	app:click('#account-row-o1 >> role:button "Account actions"')
	settle(app)
	expect(line_of(app, PAGE, "menuitem", "Rename") == nil and line_of(app, "", "menuitem", "Rename") == nil).to_be(true)
end)

test("another device's agents take sign-ins, never keys", function()
	local app = open()
	app:command("groups.remote")
	settle(app)
	expect(app:has("#account-add-key-pi")).to_be(false)
	expect(app:has('#account-add-pi-anthropic >> role:button "Add Claude account"')).to_be(true)
end)

test("switching a ChatGPT account moves the one in use within ChatGPT alone", function()
	local app = open()
	app:click('#account-row-o2 >> role:button "Account actions"')
	settle(app)
	app:click('"Switch to this account"')
	settle(app)
	expect(app:state("groups_calls")).to_contain("switch:pi:o2")
	expect(in_use(app, "o2")).to_be(true)
	expect(in_use(app, "o1")).to_be(false)
	expect(in_use(app, "c1")).to_be(true)
end)

test("Switch is offered only to an account that can be put in use and isn't; a refusal is said", function()
	local app = open()
	-- In use already: no Switch.
	app:click('#account-row-o1 >> role:button "Account actions"')
	settle(app)
	expect(line_of(app, "", "menuitem", "Switch to this account")).to_be(nil)
	app:key("Escape")
	settle(app)
	-- Refused by the engine: a toast says why.
	app:click('#account-row-o3 >> role:button "Account actions"')
	settle(app)
	app:click('"Switch to this account"')
	settle(app)
	expect(app:state("groups_calls")).to_contain("switch:pi:o3")
	expect(app:has('"Couldn\'t switch accounts"')).to_be(true)
	expect(app:has('~"no longer exists"')).to_be(true)
end)

test("Hermes' own pool is read-only, its add rows Hermes' own sign-ins and keys", function()
	local app = open()
	expect(in_use(app, "h1")).to_be(true)
	expect(app:has('#account-row-h1 >> role:button "Account actions"')).to_be(false)
	-- From the environment: never titled by its variable.
	expect(shows(app, "#account-row-h3", "API key ···9f3e")).to_be(true)
	expect(shows(app, "#account-row-h3", "OpenRouter · from environment")).to_be(true)
	expect(shows(app, "#account-row-h3", "OPENROUTER_API_KEY")).to_be(false)
	expect(app:has('#account-add-hermes-nous >> role:button "Add Nous Portal account"')).to_be(true)
	expect(app:has('#account-add-hermes-xai-oauth >> role:button "Add xAI account"')).to_be(true)
	expect(app:has('#account-add-key-hermes >> role:button "Add API key"')).to_be(true)
	app:click('#account-add-hermes-nous >> role:button "Add Nous Portal account"')
	settle(app)
	expect(app:state("groups_calls")).to_contain("sign_in_to:hermes:nous")
end)

test("an agent with one login looks as it always has", function()
	local app = open()
	expect(app:has("#account-add-key-claude-code")).to_be(false)
	expect(shows(app, PAGE, "bo@example.com")).to_be(true)
	-- (Claude Code's and Codex's foot: "Add account", as ever.)
	local _, n = string.gsub(app:tree(PAGE), 'button "Add account"', "")
	expect(n).to_be(2)
end)

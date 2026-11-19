-- Settings → Providers offers Cloud as one more choice after the computers,
-- while Cloud is on: its agents' accounts (the vault's, every Cloud machine
-- uses them), with no switch, no settings and nothing to install, and API
-- keys to save for it, each in a dialog: an Anthropic key may go through an
-- LLM gateway, its base URL judged as it's typed. Without Cloud, no such
-- choice. On the shell in ui/shell_fixture.luau, Cloud from
-- ui/cloud_stub.luau, the host's answers from ui/settings_stub.luau.

local PAGE = "#settings-page"
local NAV = "#settings-nav"
local DIALOG = "#cloud-key-dialog"

local function shows(app: any, scope: string, text: string): boolean
	return app:has(scope) and app:tree(scope):find(text, 1, true) ~= nil
end

-- Whether a text says exactly `text` under `scope` (the tree cuts long lines).
local function says(app: any, scope: string, text: string): boolean
	return app:has(`{scope} >> "{text}"`)
end

local function asked(app: any, entry: string): boolean
	for _, c in (app:state("settings_stub_calls") :: any) or {} do
		if c == entry then
			return true
		end
	end
	return false
end

-- Whether a dialog titled `title` is open (a closed one stays, hidden, as it
-- leaves: a test host's frames pass no time). On the modal layer, which the
-- whole window's tree leaves out: asked by its own selector.
local function dialog_open(app: any, title: string): boolean
	local dialog = `role:dialog "{title}"`
	if not app:has(dialog) then
		return false
	end
	local line = app:tree(dialog):match("[^\n]+")
	return line ~= nil and line:find(" hidden", 1, true) == nil
end

-- A node's line in the tree (`kind "label"` under `scope`), shown.
local function line_of(app: any, scope: string, kind: string, label: string): string?
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(` {kind} "{label}"`, 1, true) and not line:find(" hidden", 1, true) then
			return line
		end
	end
	return nil
end

local function save_disabled(app: any, title: string): boolean
	return (line_of(app, `role:dialog "{title}"`, "button", "Save") or ""):find("disabled", 1, true) ~= nil
end

local function open(with_cloud: boolean): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	if with_cloud then
		app:command("cloud_stub.offer")
	end
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Providers"`)
	app:frames(30)
	return app
end

local function on_cloud(app: any)
	app:click(`{PAGE} >> role:button "Device"`)
	app:frames(10)
	app:click('role:menuitem "Cloud"')
	app:frames(30)
end

test("with Cloud on, Providers offers Cloud: its agents' accounts, no switch or settings, and its API keys", function()
	local app = open(true)
	app:click(`{PAGE} >> role:button "Device"`)
	app:frames(10)
	expect(app:has('role:menuitem "Cloud"')).to_be(true)
	app:click('role:menuitem "Cloud"')
	app:frames(30)
	expect(asked(app, "agents.target:cloud")).to_be(true)
	expect(shows(app, PAGE, "Claude Code")).to_be(true)
	expect(shows(app, PAGE, "Codex")).to_be(true)
	expect(shows(app, PAGE, "ada@example.com")).to_be(true)
	-- Nothing to switch, set or install: its accounts alone.
	expect(app:has(`{PAGE} >> role:switch "Codex on"`)).to_be(false)
	expect(app:has(`{PAGE} >> role:button "Codex settings"`)).to_be(false)
	expect(shows(app, PAGE, "Add another agent")).to_be(false)
	-- Its API keys, a dialog each. OpenAI's goes to OpenAI: no gateway asked.
	expect(app:has(`{PAGE} >> role:button "Anthropic API key"`)).to_be(true)
	app:click(`{PAGE} >> role:button "OpenAI API key"`)
	app:frames(20)
	expect(dialog_open(app, "Add OpenAI API key")).to_be(true)
	expect(says(app, 'role:dialog "Add OpenAI API key"', "Codex uses it on every Cloud machine, from its next turn.")).to_be(true)
	expect(line_of(app, DIALOG, "radio", "LLM gateway")).to_be_nil()
	expect(save_disabled(app, "Add OpenAI API key")).to_be(true)
	app:type(`{DIALOG} >> role:textbox "API key"`, "sk-test-123")
	app:frames(5)
	app:click(`role:dialog "Add OpenAI API key" >> role:button "Save"`)
	app:frames(20)
	expect(asked(app, "agents.cloud_key:openai-key@-")).to_be(true)
	expect(dialog_open(app, "Add OpenAI API key")).to_be(false)
	expect(app:error()).to_be_nil()
	-- Back to this computer: its agents as they were, with their switches.
	app:click(`{PAGE} >> role:button "Device"`)
	app:frames(10)
	app:click('role:menuitem "MacBook Pro"')
	app:frames(30)
	expect(asked(app, "agents.target:")).to_be(true)
	expect(app:has(`{PAGE} >> role:switch "Claude Code on"`)).to_be(true)
	expect(shows(app, PAGE, "Anthropic API key")).to_be(false)
end)

test("an Anthropic key for Cloud may go through an LLM gateway: its base URL judged as it's typed, kept with the key", function()
	local app = open(true)
	on_cloud(app)
	app:click(`{PAGE} >> role:button "Anthropic API key"`)
	app:frames(20)
	local title = "Add Anthropic API key"
	expect(dialog_open(app, title)).to_be(true)
	app:type(`{DIALOG} >> role:textbox "API key"`, "sk-ant-test-1")
	app:frames(5)
	-- Anthropic's own API first: no base URL asked.
	expect(line_of(app, DIALOG, "textbox", "Base URL")).to_be_nil()
	expect(save_disabled(app, title)).to_be(false)
	app:click(`{DIALOG} >> role:radio "LLM gateway"`)
	app:frames(10)
	expect(line_of(app, DIALOG, "textbox", "Base URL") ~= nil).to_be(true)
	-- No address yet: Save waits, and nothing is called wrong.
	expect(save_disabled(app, title)).to_be(true)
	expect(says(app, DIALOG, "Your gateway's https:// address, without /v1.")).to_be(true)
	-- Half an address: still quiet.
	app:fill(`{DIALOG} >> role:textbox "Base URL"`, "https:/")
	app:frames(5)
	expect(says(app, DIALOG, "Your gateway's https:// address, without /v1.")).to_be(true)
	-- Plain http: refused in the vault's words.
	app:fill(`{DIALOG} >> role:textbox "Base URL"`, "http://llm.example.com")
	app:frames(5)
	expect(says(app, DIALOG, "Use an https:// base URL — the key is sent with every request.")).to_be(true)
	expect(save_disabled(app, title)).to_be(true)
	-- With /v1 on the end, Claude Code would ask for /v1/v1/messages.
	app:fill(`{DIALOG} >> role:textbox "Base URL"`, "https://llm.example.com/v1")
	app:frames(5)
	expect(says(app, DIALOG, "Leave /v1 off the base URL — Claude Code adds /v1/messages itself.")).to_be(true)
	expect(save_disabled(app, title)).to_be(true)
	-- A gateway: the route each request takes, then kept with the key.
	app:fill(`{DIALOG} >> role:textbox "Base URL"`, "https://llm.example.com/anthropic/")
	app:frames(5)
	expect(says(app, DIALOG, "Claude Code sends each request to https://llm.example.com/anthropic/v1/messages")).to_be(true)
	expect(save_disabled(app, title)).to_be(false)
	app:click(`role:dialog "{title}" >> role:button "Save"`)
	app:frames(20)
	expect(asked(app, "agents.cloud_key:anthropic-key@https://llm.example.com/anthropic")).to_be(true)
	expect(dialog_open(app, title)).to_be(false)
	expect(app:error()).to_be_nil()
	-- Opened again: nothing typed is kept, Anthropic's own API chosen.
	app:click(`{PAGE} >> role:button "Anthropic API key"`)
	app:frames(20)
	expect(line_of(app, DIALOG, "textbox", "Base URL")).to_be_nil()
	expect(save_disabled(app, title)).to_be(true)
end)

test("a key the vault refuses keeps the dialog open, saying why, the key as typed", function()
	local app = open(true)
	on_cloud(app)
	app:click(`{PAGE} >> role:button "Anthropic API key"`)
	app:frames(20)
	app:command("settings_stub.refuse_next")
	app:type(`{DIALOG} >> role:textbox "API key"`, "sk-ant-refused")
	app:key("Enter")
	app:frames(20)
	expect(asked(app, "agents.cloud_key:anthropic-key@-")).to_be(true)
	expect(dialog_open(app, "Add Anthropic API key")).to_be(true)
	expect(says(app, DIALOG, "refused")).to_be(true)
	expect(app:text(`{DIALOG} >> role:textbox "API key"`)).to_be("sk-ant-refused")
	expect(app:error()).to_be_nil()
end)

test("without Cloud, Providers offers no Cloud", function()
	local app = open(false)
	app:click(`{PAGE} >> role:button "Device"`)
	app:frames(10)
	expect(app:has('role:menuitem "Cloud"')).to_be(false)
end)

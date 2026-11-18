-- Settings → Providers offers Cloud as one more choice after the computers,
-- while Cloud is on: its agents' accounts (the vault's, every Cloud machine
-- uses them), with no switch, no settings and nothing to install, and API
-- keys to save for it. Without Cloud, no such choice. On the shell in
-- ui/shell_fixture.luau, Cloud from ui/cloud_stub.luau.

local PAGE = "#settings-page"
local NAV = "#settings-nav"

local function shows(app: any, scope: string, text: string): boolean
	return app:tree(scope):find(text, 1, true) ~= nil
end

local function asked(app: any, entry: string): boolean
	for _, c in (app:state("settings_stub_calls") :: any) or {} do
		if c == entry then
			return true
		end
	end
	return false
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
	-- Its API keys, saved for Cloud.
	expect(shows(app, PAGE, "Anthropic API key")).to_be(true)
	expect(shows(app, PAGE, "OpenAI API key")).to_be(true)
	expect(shows(app, PAGE, "Meta API key")).to_be(true)
	-- Every agent signs in on Cloud as it does on a computer, Muse Code
	-- included.
	expect(shows(app, PAGE, "Muse Code")).to_be(true)
	expect(shows(app, PAGE, "Connect a Muse Code account")).to_be(true)
	expect(shows(app, PAGE, "Connect a Claude Code account")).to_be(true)
	app:type(`{PAGE} >> role:textbox "OpenAI API key"`, "sk-test-123")
	app:frames(5)
	app:click(`{PAGE} >> role:button "Save" [2]`)
	app:frames(10)
	expect(asked(app, "agents.cloud_key")).to_be(true)
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

test("without Cloud, Providers offers no Cloud", function()
	local app = open(false)
	app:click(`{PAGE} >> role:button "Device"`)
	app:frames(10)
	expect(app:has('role:menuitem "Cloud"')).to_be(false)
end)

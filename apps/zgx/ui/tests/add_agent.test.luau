-- Settings → Providers → Add an agent: the ACP registry's agents, searched;
-- one read before it's added (who makes it, how it runs on this Mac, how
-- its download is checked, what it may do); the add in place, done or
-- failed; an agent this Mac can't run yet saying why; and once added, its
-- card among the providers (its icon, a sign-in in place of accounts), the
-- registry's newer version offered, and removing it. Settings on the shell
-- in ui/shell_fixture.luau, the registry answered by ui/registry_stub.luau.

local PAGE = "#settings-page"
local NAV = "#settings-nav"

local function shows(app: any, text: string): boolean
	return app:tree(PAGE):find(text, 1, true) ~= nil
end

local function calls(app: any): string
	local ok, list = pcall(function()
		return app:state("registry_stub_calls")
	end)
	return if ok and list then table.concat(list, ",") else ""
end

-- Settings → Providers, on the registry's page.
local function registry(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Providers"`)
	app:frames(30)
	app:click(`{PAGE} >> "Add another agent"`)
	app:frames(40)
	return app
end

local function open(app: any, name: string)
	app:click(`{PAGE} >> "{name}"`)
	app:frames(40)
end

test("Add an agent: a row on Providers, the registry listed, the built-in ones staying put", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Providers"`)
	app:frames(30)
	expect(shows(app, "Add another agent")).to_be(true)
	-- (The built-in Claude Agent isn't counted.)
	expect(shows(app, "4 more agents in the ACP registry.")).to_be(true)
	app:click(`{PAGE} >> "Add another agent"`)
	app:frames(40)
	expect(shows(app, "Add an agent")).to_be(true)
	expect(shows(app, "Every agent that speaks the Agent Client Protocol: 5 today.")).to_be(true)
	for _, name in { "Claude Agent", "fast-agent", "Gemini CLI", "Kimi CLI", "Poolside" } do
		expect(shows(app, name)).to_be(true)
	end
	expect(shows(app, "Built in")).to_be(true)
	-- A built-in agent doesn't open.
	open(app, "Claude Agent")
	expect(shows(app, "Every agent that speaks")).to_be(true)
	-- And back.
	app:click(`{PAGE} >> "Providers"`)
	app:frames(40)
	expect(shows(app, "Your agents") or shows(app, "More agents")).to_be(true)
	expect(shows(app, "Every agent that speaks")).to_be(false)
end)

test("Add an agent: a search narrows the rows in place, and one that finds nothing says so", function()
	local app = registry()
	app:fill(`{PAGE} >> role:textbox "Search agents"`, "moonshot")
	app:frames(30)
	expect(shows(app, "Kimi CLI")).to_be(true)
	expect(shows(app, "Gemini CLI")).to_be(false)
	app:fill(`{PAGE} >> role:textbox "Search agents"`, "zzz")
	app:frames(30)
	expect(shows(app, "Nothing called “zzz”")).to_be(true)
	app:click(`{PAGE} >> "Clear search"`)
	app:frames(30)
	expect(shows(app, "Gemini CLI")).to_be(true)
end)

test("Add an agent: one agent read before it's added, then added in place and among the providers", function()
	local app = registry()
	open(app, "Gemini CLI")
	for _, text in {
		"Google's official CLI for Gemini.",
		"Made by",
		"Google",
		"0.66.0",
		"Apache-2.0",
		"github.com/google-gemini/gemini-cli",
		"With Node.js, which this Mac has.",
		"Version 0.66.0 exactly, from npm",
		"Gemini CLI's own, once it's added.",
		"Gemini CLI runs on this Mac with your access.",
	} do
		expect(shows(app, text)).to_be(true)
	end
	app:click(`{PAGE} >> role:button "Add to this Mac"`)
	app:frames(40)
	expect(calls(app):find("registry.add:gemini", 1, true) ~= nil).to_be(true)
	expect(shows(app, "Gemini CLI is ready")).to_be(true)
	expect(shows(app, "It's in your providers on this Mac, switched on.")).to_be(true)
	app:click(`{PAGE} >> "Open Providers"`)
	app:frames(40)
	-- Its card: a sign-in where accounts would be, no "Connect an account".
	expect(shows(app, "Sign in to Gemini CLI")).to_be(true)
	expect(shows(app, "Connect a Gemini CLI account")).to_be(false)
end)

test("Add an agent: a binary says it's checked, and a failed add says why and changes nothing", function()
	local app = registry()
	open(app, "Kimi CLI")
	expect(shows(app, "Downloads its build for this Mac from github.com.")).to_be(true)
	expect(shows(app, "Against the checksum the registry publishes.")).to_be(true)
	app:command("registry_stub.fail_next")
	app:click(`{PAGE} >> role:button "Add to this Mac"`)
	app:frames(40)
	expect(shows(app, "Couldn't add Kimi CLI")).to_be(true)
	expect(shows(app, "Kimi CLI's download doesn't match the registry's checksum")).to_be(true)
	-- Another go adds it.
	app:click(`{PAGE} >> "Try again"`)
	app:frames(40)
	expect(shows(app, "Kimi CLI is ready")).to_be(true)
end)

test("Add an agent: no checksum is said plainly, and one the Mac can't run yet can't be added", function()
	local app = registry()
	open(app, "Poolside")
	expect(shows(app, "The registry publishes no checksum for this build.")).to_be(true)
	app:click(`{PAGE} >> "Add an agent"`)
	app:frames(40)
	open(app, "fast-agent")
	expect(shows(app, "Needs uv on this Mac, which isn't there yet.")).to_be(true)
	app:click(`{PAGE} >> role:button "Add to this Mac"`)
	app:frames(30)
	expect(calls(app):find("registry.add:fast-agent", 1, true)).to_be(nil)
end)

test("Add an agent: an added agent offers the registry's newer version, and is removed", function()
	local app = registry()
	open(app, "Gemini CLI")
	app:click(`{PAGE} >> role:button "Add to this Mac"`)
	app:frames(40)
	app:command("registry_stub.newer")
	-- On Providers, its card offers the update, as any agent's.
	app:click(`{PAGE} >> "Open Providers"`)
	app:frames(40)
	expect(app:has(`{PAGE} >> role:button "Update"`)).to_be(true)
	app:click(`{PAGE} >> "Add another agent"`)
	app:frames(40)
	expect(shows(app, "Update")).to_be(true)
	open(app, "Gemini CLI")
	expect(shows(app, "On this Mac")).to_be(true)
	expect(shows(app, "Version 0.66.0; the registry has 0.67.0.")).to_be(true)
	app:click(`{PAGE} >> role:button "Update to 0.67.0"`)
	app:frames(40)
	expect(calls(app):find("registry.add:gemini,registry.load", 1, true) ~= nil or calls(app):find("registry.add:gemini", 1, true) ~= nil).to_be(true)
	app:click(`{PAGE} >> "Add an agent"`)
	app:frames(40)
	open(app, "Gemini CLI")
	expect(shows(app, "Version 0.67.0, the registry's latest.")).to_be(true)
	app:click(`{PAGE} >> role:button "Remove"`)
	app:frames(30)
	expect(app:has(`"Remove Gemini CLI from this Mac?"`)).to_be(true)
	app:click(`role:dialog >> role:button "Remove"`)
	app:frames(40)
	expect(calls(app):find("registry.remove:gemini", 1, true) ~= nil).to_be(true)
	expect(shows(app, "Add to this Mac")).to_be(true)
end)

test("Add an agent: an added agent signs in its own way, its device code said in the sign-in's row", function()
	local app = registry()
	open(app, "Gemini CLI")
	app:click(`{PAGE} >> role:button "Add to this Mac"`)
	app:frames(40)
	app:click(`{PAGE} >> "Open Providers"`)
	app:frames(40)
	app:click(`{PAGE} >> "Sign in to Gemini CLI"`)
	app:frames(30)
	expect(calls(app):find("agents.sign_in:acp.gemini", 1, true) ~= nil).to_be(true)
	expect(shows(app, "Signing in to Gemini CLI")).to_be(true)
	-- Its code, said on its own, and what to do with it.
	-- (The line no longer carries it: the code shown is the row's own.)
	expect(shows(app, "WDJB-MJHT")).to_be(true)
	expect(shows(app, "Your code is")).to_be(false)
	expect(shows(app, "Check this code matches the one on the page that opened")).to_be(true)
	-- Its page, opened again from the card (a tab closed).
	app:click(`{PAGE} >> role:button "Open the page again"`)
	app:frames(10)
	expect(calls(app):find("agents.open_page:acp.gemini", 1, true) ~= nil).to_be(true)
end)

test("Add an agent: one of your own, from its command, joins the providers", function()
	local app = registry()
	open(app, "Run your own agent")
	expect(shows(app, "The program and its arguments")).to_be(true)
	app:fill(`{PAGE} >> role:textbox "Name"`, "Halcyon agent")
	app:fill(`{PAGE} >> role:textbox "Command"`, "/nowhere/agent --acp")
	app:frames(10)
	app:click(`{PAGE} >> role:button "Add to this Mac"`)
	app:frames(30)
	expect(shows(app, "Couldn't add your agent")).to_be(true)
	expect(shows(app, "/nowhere/agent isn't on this device")).to_be(true)
	app:fill(`{PAGE} >> role:textbox "Command"`, "~/code/halcyon/bin/agent --acp")
	app:frames(10)
	app:click(`{PAGE} >> role:button "Add to this Mac"`)
	app:frames(40)
	expect(calls(app):find("registry.add_own:Halcyon agent:~/code/halcyon/bin/agent --acp", 1, true) ~= nil).to_be(true)
	-- Back on Providers, its card, and not among the registry's rows.
	expect(shows(app, "Sign in to Halcyon agent")).to_be(true)
	app:click(`{PAGE} >> "Add another agent"`)
	app:frames(40)
	expect(shows(app, "Halcyon agent")).to_be(false)
end)

test("Add an agent: an added agent's environment, in its settings, kept on the device", function()
	local app = registry()
	open(app, "Gemini CLI")
	app:click(`{PAGE} >> role:button "Add to this Mac"`)
	app:frames(40)
	app:click(`{PAGE} >> "Open Providers"`)
	app:frames(40)
	app:click(`{PAGE} >> role:button "Gemini CLI settings"`)
	app:frames(30)
	expect(app:has(`role:dialog >> "Environment"`)).to_be(true)
	expect(app:has(`role:dialog >> "Remove from this Mac"`)).to_be(true)
	-- Its updates as any agent's: the policy, the registry their feed.
	expect(app:has(`role:dialog >> "Updates"`)).to_be(true)
	app:fill(`role:dialog >> role:textbox "Environment"`, "GEMINI_API_KEY=sk-1")
	-- Kept a moment after typing stops.
	local kept = false
	for _ = 1, 40 do
		kept = calls(app):find("registry.env_set:gemini:GEMINI_API_KEY=sk-1", 1, true) ~= nil
		if kept then
			break
		end
		app:wait('"never shown, only waited past"', { timeout_ms = 50 })
	end
	expect(kept).to_be(true)
	-- A line that isn't KEY=VALUE says so under the field.
	app:fill(`role:dialog >> role:textbox "Environment"`, "not a pair")
	local said = false
	for _ = 1, 40 do
		said = app:has(`role:dialog >> "line 1 isn't KEY=VALUE"`)
		if said then
			break
		end
		app:wait('"never shown, only waited past"', { timeout_ms = 50 })
	end
	expect(said).to_be(true)
end)

test("Add an agent: a sign-in that asks for typed input runs in a terminal on the agent's device", function()
	local app = registry()
	open(app, "Gemini CLI")
	app:click(`{PAGE} >> role:button "Add to this Mac"`)
	app:frames(40)
	app:click(`{PAGE} >> "Open Providers"`)
	app:frames(40)
	app:command("registry_stub.needs_terminal")
	app:frames(30)
	expect(shows(app, "Couldn't sign in to Gemini CLI")).to_be(true)
	expect(shows(app, "This sign-in asks for something only a terminal")).to_be(true)
	app:click(`{PAGE} >> role:button "Open a terminal"`)
	app:frames(20)
	expect(app:has(`role:dialog >> "Sign in to Gemini CLI"`)).to_be(true)
	-- (It opens a beat after the dialog does.)
	expect(app:wait("#sign-in-terminal", { timeout_ms = 2000 })).to_be(true)
	expect(calls(app):find("terminal.start:acp-sign-in:acp.gemini", 1, true) ~= nil).to_be(true)
	-- Its sign-in typed in for you.
	expect(calls(app):find("terminal.write:/agents/gemini/node_modules/.bin/gemini --acp login\n", 1, true) ~= nil).to_be(true)
	-- Done: the terminal ends, and the failed sign-in goes.
	app:click(`role:dialog >> role:button "Done"`)
	app:frames(40)
	expect(calls(app):find("terminal.close", 1, true) ~= nil).to_be(true)
	expect(shows(app, "Couldn't sign in to Gemini CLI")).to_be(false)
	expect(shows(app, "Sign in to Gemini CLI")).to_be(true)
end)


test("Add an agent: an agent signed in already says so, as whom, and signs out", function()
	-- (What the user hit on Nov 3: Factory Droid was signed in, and the row
	-- only ever offered a sign-in that ended at once.)
	local app = registry()
	open(app, "Gemini CLI")
	app:click(`{PAGE} >> role:button "Add to this Mac"`)
	app:frames(40)
	app:click(`{PAGE} >> "Open Providers"`)
	app:frames(40)
	expect(shows(app, "Sign in to Gemini CLI")).to_be(true)
	app:command("registry_stub.signed_in")
	app:frames(20)
	expect(shows(app, "Signed in to Gemini CLI")).to_be(true)
	expect(shows(app, "Signed in as ada@example.com.")).to_be(true)
	expect(shows(app, "Sign in to Gemini CLI")).to_be(false)
	app:click(`{PAGE} >> role:button "Sign out"`)
	app:frames(20)
	expect(calls(app):find("agents.sign_out:acp.gemini", 1, true) ~= nil).to_be(true)
	expect(shows(app, "Sign in to Gemini CLI")).to_be(true)
end)

test("Add an agent: each way an agent offers to sign in is a row, and a key it reads is pasted and kept", function()
	local app = registry()
	open(app, "Gemini CLI")
	app:click(`{PAGE} >> role:button "Add to this Mac"`)
	app:frames(40)
	app:click(`{PAGE} >> "Open Providers"`)
	app:frames(40)
	app:command("registry_stub.methods")
	app:frames(20)
	-- Its own sign-in first, then the other way it signs in itself, then
	-- the key.
	expect(shows(app, "Sign in to Gemini CLI")).to_be(true)
	expect(shows(app, "Vertex AI")).to_be(true)
	expect(shows(app, "Log in with Google")).to_be(false)
	app:click(`{PAGE} >> "Vertex AI"`)
	app:frames(10)
	expect(calls(app):find("agents.sign_in_with:acp.gemini:vertex-ai", 1, true) ~= nil).to_be(true)
	app:click(`{PAGE} >> "Use Gemini API key"`)
	app:frames(20)
	expect(shows(app, "Paste your Gemini API key")).to_be(true)
	app:fill(`{PAGE} >> role:textbox "GEMINI_API_KEY"`, "  AIza-test-key  ")
	app:frames(5)
	app:click(`{PAGE} >> role:button "Save"`)
	app:frames(20)
	expect(calls(app):find("agents.set_key:acp.gemini:GEMINI_API_KEY:13", 1, true) ~= nil).to_be(true)
	-- (Kept, and the field gone: the key isn't shown again.)
	expect(shows(app, "Paste your Gemini API key")).to_be(false)
	expect(shows(app, "AIza-test-key")).to_be(false)
end)

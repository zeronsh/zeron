-- Settings → General → "Keep Zeron running after you quit": the engine's
-- own setting (crates/engine/src/keep_running.rs), on until turned off; a
-- flip goes to the engine and the row says what quitting does now. The
-- question asked on quitting with work running is the host's
-- (src/quit_prompt.rs), in the app's words: handed over as the language
-- changes. On the shell in ui/shell_fixture.luau.

local PAGE = "#settings-page"
local LABEL = "Keep Zeron running after you quit"

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

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	return app
end

test("on until turned off, and the row says what quitting does", function()
	local app = open()
	local switch = `{PAGE} >> role:switch "{LABEL}"`
	expect(asked(app, "settings.engine:GetBackgroundSettings")).to_be(true)
	expect(app:tree(switch)).never.to_contain("unchecked")
	expect(shows(app, PAGE, "Chats keep working, and other devices can reach them.")).to_be(true)
	app:click(switch)
	app:frames(10)
	expect(asked(app, "settings.engine:SetBackgroundSettings")).to_be(true)
	expect(app:tree(switch)).to_contain("unchecked")
	expect(shows(app, PAGE, "When you quit, Zeron stops once its chats are done.")).to_be(true)
	-- Refused: it goes back as it was.
	app:command("settings_stub.refuse_next")
	app:click(switch)
	app:frames(10)
	expect(app:tree(switch)).to_contain("unchecked")
end)

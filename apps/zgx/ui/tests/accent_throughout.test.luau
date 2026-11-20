-- "Accent color throughout" (Settings → Appearance, ds/look `accented`):
-- on by default, the accent in more places, as the shipping app has it (a
-- link, the working dot, a find's highlight, an @file token, a list
-- marker); off, each is what it was. Kept across a relaunch. On the shell
-- in ui/shell_fixture.luau, the window's theme read through
-- settings_stub.read_theme.

local ON = "ds/look.luau::accent_throughout"
local ROW = 'role:switch "Accent color throughout"'

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	return app
end

local function theme(app: any): { [string]: any }
	app:command("settings_stub.read_theme")
	app:frames(2)
	return app:state("settings_stub_window_theme")
end

test("it's on by default, and Appearance shows it on", function()
	local app = open()
	expect(app:state(ON)).to_be(true)
	app:command("shell.settings")
	app:frames(30)
	app:click('#shell-settings >> role:tab "Appearance"')
	app:frames(30)
	expect(app:has(ROW)).to_be(true)
	expect(app:tree(ROW)).to_contain(" checked")
end)

test("on, a link, the working dot and a find's highlight take the accent; off, what they were", function()
	local app = open()
	local t = theme(app)
	-- (The accent's own token is the theme's accent.)
	expect(t.accented_ink).to_be(t.accent_ink)
	expect(t.link).to_be("$zeron.accented.ink")
	expect(t.working_dot).to_be("$zeron.accented.ink")
	expect(t.working).to_be("$zeron.accented.ink")
	expect(t.find).to_be("$zeron.accented.find")
	expect(t.file_token).to_be("$zeron.accented.ink")
	expect(t.marker).to_be("$zeron.accented.ink")
	-- Switched off in Appearance: nothing overridden, the theme's own.
	app:command("shell.settings")
	app:frames(30)
	app:click('#shell-settings >> role:tab "Appearance"')
	app:frames(30)
	app:click(ROW)
	app:frames(10)
	expect(app:state(ON)).to_be(false)
	t = theme(app)
	expect(t.link).to_be_nil()
	expect(t.working_dot).to_be_nil()
	expect(t.working).to_be_nil()
	expect(t.find).to_be_nil()
	expect(t.file_token).to_be_nil()
	expect(t.marker).to_be_nil()
	expect(app:error()).to_be_nil()
end)

test("off, it stays off after a relaunch", function()
	local app = open()
	app:set_state(ON, false)
	app:frames(5)
	local again = open()
	expect(again:state(ON)).to_be(false)
	expect(theme(again).working_dot).to_be_nil()
end)

test("a conversation with links, lists, quotes and inline code draws without an error either way", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:type("role:textbox", "See [the docs](https://example.com), `code`:\n\n- one\n- two\n\n1. first\n\n> quoted")
	app:key("Enter")
	app:frames(30)
	expect(app:wait('~"quoted"', { timeout_ms = 2000 })).to_be(true)
	expect(app:error()).to_be_nil()
	app:set_state(ON, false)
	app:frames(10)
	expect(app:error()).to_be_nil()
	app:set_state(ON, true)
	app:frames(10)
	expect(app:error()).to_be_nil()
end)

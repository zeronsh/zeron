-- Reload Zeron (the app relaunched whole, src/relaunch.rs) and Reload
-- interface (its scripts evaluated again in place): each a command of the
-- shell's and a row of the palette, found by what someone would type, and
-- each asking the host. On the shell in ui/shell_fixture.luau, the host's
-- calls answered by ui/settings_stub.luau.

local function calls(app: any): { string }
	return (app:state("settings_stub_calls") :: any) or {}
end

local function asked(app: any, entry: string): boolean
	return table.find(calls(app), entry) ~= nil
end

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	return app
end

-- `text` in the palette's field (in place of what was there), the palette opened first if it's closed.
local function search(app: any, text: string)
	if app:state("shell_palette_open") ~= true then
		app:command("shell.palette")
		app:frames(20)
	end
	app:fill('role:textbox "Search"', text)
	app:frames(30)
end

test("the palette finds Reload Zeron by reload, restart and relaunch", function()
	local app = open()
	search(app, "reload")
	expect(app:has('#palette-list >> role:option "Reload Zeron"')).to_be(true)
	expect(app:has('#palette-list >> role:option "Reload interface"')).to_be(true)
	for _, word in { "restart", "relaunch" } do
		search(app, word)
		expect(app:has('#palette-list >> role:option "Reload Zeron"')).to_be(true)
	end
end)

test("picking Reload Zeron asks the host to relaunch the app", function()
	local app = open()
	search(app, "Reload Zeron")
	expect(asked(app, "app.relaunch")).to_be(false)
	app:click('#palette-list >> role:option "Reload Zeron"')
	app:frames(20)
	expect(asked(app, "app.relaunch")).to_be(true)
	expect(asked(app, "app.reload_interface")).to_be(false)
end)

test("picking Reload interface asks the host to reload the scripts, not the app", function()
	local app = open()
	search(app, "Reload interface")
	app:click('#palette-list >> role:option "Reload interface"')
	app:frames(20)
	expect(asked(app, "app.reload_interface")).to_be(true)
	expect(asked(app, "app.relaunch")).to_be(false)
end)

test("each is a command of its own (zgx inspect command, a key Settings gives it)", function()
	local app = open()
	app:command("shell.reload")
	app:frames(5)
	expect(asked(app, "app.relaunch")).to_be(true)
	app:command("shell.reload_interface")
	app:frames(5)
	expect(asked(app, "app.reload_interface")).to_be(true)
end)

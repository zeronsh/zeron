-- A chat whose device is away (the engine publishes the device `online =
-- false`: its presence heartbeat went quiet) wears a wifi-off glyph in its
-- sidebar row, saying which device and what it means; back online, it's
-- gone. View options → Show → Unreachable chats (on by default) turns it
-- off. On the shell in ui/shell_fixture.luau (devbox hosts Borealis's chats).

local SIDE = "#shell-sidebar"
local SHOW = "screens/shell/sidebar.luau::shell_sidebar_show_unreachable"
local SAYS = "devbox is offline — this chat can't run until it reconnects"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	return app
end

local function glyphs(app: any): number
	local n = 0
	for line in app:tree(SIDE):gmatch("[^\n]+") do
		if line:find(SAYS, 1, true) then
			n += 1
		end
	end
	return n
end

test("it's on by default, and a device that's online marks nothing", function()
	local app = open()
	expect(app:state(SHOW)).to_be(true)
	expect(glyphs(app)).to_be(0)
end)

test("every chat of an offline device says so; the others' don't; back online, none", function()
	local app = open()
	app:command("fixture.devbox_offline")
	app:frames(40)
	-- (Borealis's two chats, on devbox.)
	expect(glyphs(app)).to_be(2)
	app:command("fixture.devbox_online")
	app:frames(40)
	expect(glyphs(app)).to_be(0)
end)

test("Show → Unreachable chats off hides it; on, it's back", function()
	local app = open()
	app:command("fixture.devbox_offline")
	app:frames(40)
	app:click(`{SIDE} >> role:button "Sidebar view options"`)
	app:frames(20)
	expect(app:has('role:checkbox "Unreachable chats"')).to_be(true)
	app:click('role:checkbox "Unreachable chats"')
	app:frames(30)
	expect(app:state(SHOW)).to_be(false)
	app:key("Escape")
	app:frames(30)
	expect(glyphs(app)).to_be(0)
	app:set_state(SHOW, true)
	app:frames(30)
	expect(glyphs(app)).to_be(2)
end)

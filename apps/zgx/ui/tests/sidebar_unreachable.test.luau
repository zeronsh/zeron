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

-- A row with no glyph to show has none to leave a gap for: its pull request
-- badge and its subagents pill stand one gap (8 points) apart, as they did
-- before the glyph's place in the line. On the sidebar in
-- ui/sidebar_gap_fixture.luau.
test("a row that's reachable leaves no gap where its glyph would be", function()
	local app = mount("sidebar_gap_fixture.luau")
	app:resize(600, 700)
	app:frames(30)
	local pr_right, pill_left = 0, 0
	for line in app:tree('#wide >> role:button "Write the docs"'):gmatch("[^\n]+") do
		local x, w = line:match(" (%-?%d+),%-?%d+ (%d+)x%d+")
		if line:find("Pull request", 1, true) and x then
			pr_right = tonumber(x) + tonumber(w)
		elseif line:find("subagent", 1, true) and not line:find("text ", 1, true) and x then
			pill_left = tonumber(x) :: number
		end
	end
	expect(pr_right > 0 and pill_left > 0).to_be(true)
	expect(pill_left - pr_right).to_be(8)
end)

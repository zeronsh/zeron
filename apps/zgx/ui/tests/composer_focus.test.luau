-- The composer takes the keys as a chat is arrived at: back from Settings
-- (any page, with Escape), a sidebar or Activity row, Back and Forward, ⌘N,
-- the palette opening a chat, a split's pane taking the focus. Not over a
-- field given the keys after (a rename). And the `/` list, hidden while
-- Settings stood over the page, shows again. On the shell in
-- ui/shell_fixture.luau.

local BOX = 'role:textbox "Message"'
local SIDE = "#shell-sidebar"

local function settle(app: any)
	app:frames(30)
end

local function shell(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	settle(app)
	app:click(`{SIDE} >> role:button "Fix the build"`)
	settle(app)
	return app
end

local function focused(app: any): string
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(" focused", 1, true) then
			return line
		end
	end
	return ""
end

local function shown(app: any): boolean
	for line in app:tree():gmatch("[^\n]+") do
		if line:find("mention-popover", 1, true) and not line:find(" hidden", 1, true) then
			return true
		end
	end
	return false
end

-- (The keys given back to the window first: Escape in the composer.)
local function away(app: any)
	app:click(`#chat-composer >> {BOX}`)
	app:key("Escape")
	settle(app)
	expect(focused(app)).never.to_contain("textbox")
end

local function has_keys(app: any): boolean
	return focused(app):find('textbox "Message"', 1, true) ~= nil
end

test("4. in the shell: `/settings` taken, Escape out of Settings, then `/`: the list opens in the composer, which has the keys", function()
	local app = shell()
	app:type(BOX, "testing /set")
	settle(app)
	expect(shown(app)).to_be(true)
	app:key("Enter")
	settle(app)
	expect(focused(app)).to_contain("shell-settings")
	app:key("Escape")
	settle(app)
	expect(focused(app)).to_contain('textbox "Message"')
	app:type(BOX, "/")
	settle(app)
	expect(shown(app)).to_be(true)
end)

test("6. Settings over the page and back: the list shows again, not put away", function()
	local app = shell()
	app:type(BOX, "testing /set")
	settle(app)
	expect(shown(app)).to_be(true)
	app:command("shell.settings")
	settle(app)
	expect(shown(app)).to_be(false)
	app:key("Escape")
	settle(app)
	expect(shown(app)).to_be(true)
	expect(app:text(BOX)).to_be("testing /set")
end)

test("Escape out of Settings, from its first page or another, gives the composer the keys", function()
	local app = shell()
	away(app)
	app:command("shell.settings")
	settle(app)
	expect(has_keys(app)).to_be(false)
	app:key("Escape")
	settle(app)
	expect(has_keys(app)).to_be(true)
	away(app)
	app:command("shell.settings")
	settle(app)
	app:click('#shell-settings >> role:tab "Appearance"')
	settle(app)
	app:key("Escape")
	settle(app)
	expect(app:state("shell_route_path")).to_be("/session/a1")
	expect(has_keys(app)).to_be(true)
end)

test("a sidebar row gives that chat's composer the keys", function()
	local app = shell()
	expect(has_keys(app)).to_be(true)
	away(app)
	app:click(`{SIDE} >> role:button "Write the docs"`)
	settle(app)
	expect(app:state("shell_route_path")).to_be("/session/a2")
	expect(has_keys(app)).to_be(true)
end)

test("an Activity row gives that chat's composer the keys", function()
	local app = shell()
	app:set_state("screens/shell/activity.luau::shell_sidebar_activity", true)
	app:frames(40)
	away(app)
	app:click(`{SIDE} >> role:button "Tidy the tests"`)
	settle(app)
	expect(app:state("shell_route_path")).to_be("/session/a3")
	expect(has_keys(app)).to_be(true)
end)

test("Back and Forward give the composer the keys", function()
	local app = shell()
	app:click(`{SIDE} >> role:button "Write the docs"`)
	settle(app)
	away(app)
	app:command("router.back")
	settle(app)
	expect(app:state("shell_route_path")).to_be("/session/a1")
	expect(has_keys(app)).to_be(true)
	away(app)
	app:command("router.forward")
	settle(app)
	expect(app:state("shell_route_path")).to_be("/session/a2")
	expect(has_keys(app)).to_be(true)
end)

test("⌘N, a new chat, gives its composer the keys", function()
	local app = shell()
	away(app)
	app:command("shell.new_chat")
	settle(app)
	expect(app:state("shell_route_path")).never.to_contain("/session/")
	expect(has_keys(app)).to_be(true)
end)

test("the palette opening a chat gives its composer the keys", function()
	local app = shell()
	away(app)
	app:command("shell.palette")
	settle(app)
	app:type('role:textbox "Search"', "Ship")
	settle(app)
	app:key("Enter")
	settle(app)
	expect(app:state("shell_route_path")).to_be("/session/b1")
	expect(has_keys(app)).to_be(true)
end)

test("a split's pane taking the focus gives the composer the keys", function()
	local app = mount("shell_fixture.luau")
	app:resize(1500, 800)
	settle(app)
	app:set_state("inset_split_trees", { { kind = "row", children = { { kind = "leaf", id = "a1" }, { kind = "leaf", id = "a2" } } } })
	app:click(`{SIDE} >> role:button "Fix the build"`)
	app:frames(60)
	away(app)
	app:click('#lane-split-a2 >> #transcript')
	settle(app)
	expect(app:state("fixture_open_chat")).to_be("a2")
	expect(has_keys(app)).to_be(true)
end)

test("a rename's field opened on another chat keeps the keys", function()
	local app = shell()
	app:right_click(`{SIDE} >> role:button "Write the docs"`)
	settle(app)
	app:click('role:menuitem "Rename"')
	settle(app)
	expect(focused(app)).to_contain("textbox")
	expect(has_keys(app)).to_be(false)
	app:frames(60)
	expect(has_keys(app)).to_be(false)
end)

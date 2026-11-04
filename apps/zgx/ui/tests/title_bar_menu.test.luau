-- The app's menus in the title bar where the system has none (Linux,
-- Windows): a ☰ before the sidebar's toggle, and Alt turning the controls
-- into the menus' titles in place. On ui/titlebar_menu_fixture.luau, which
-- serves the menus as the host does.

local function open(): any
	local app = mount("titlebar_menu_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
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

local function x_of(app: any, target: string): number
	local line = app:tree(target, { depth = 0 }):match("[^\n]+") or ""
	return tonumber(line:match(" (%-?[%d%.]+),%-?[%d%.]+ ")) or -1
end

test("the ☰ stands before the sidebar's toggle and opens every menu as a submenu", function()
	local app = open()
	expect(app:has('#titlebar >> role:button "Menu"')).to_be(true)
	expect(x_of(app, '#titlebar >> role:button "Menu"') < x_of(app, '#titlebar >> role:button "Toggle sidebar"')).to_be(true)
	app:click('#titlebar >> role:button "Menu"')
	app:frames(20)
	expect(app:has('role:menuitem "File"')).to_be(true)
	expect(app:has('role:menuitem "View"')).to_be(true)
	expect(app:has('role:menuitem "Go"')).to_be(true)
end)

test("no ☰ where the host serves no menus (macOS)", function()
	local app = open()
	app:command("fixture.no_menus")
	app:frames(20)
	expect(app:has('#titlebar >> role:button "Menu"')).to_be(false)
	expect(app:has('#titlebar >> role:button "Toggle sidebar"')).to_be(true)
end)

test("Alt turns the controls into the menus' titles in place, the first focused, and back", function()
	local app = open()
	expect(app:has("#titlebar-menus")).to_be(false)
	app:command("fixture.alt")
	app:frames(20)
	expect(app:has("#titlebar-menus")).to_be(true)
	expect(focused(app):find('"File"', 1, true)).never.to_be_nil()
	app:key("ArrowRight")
	app:frames(4)
	expect(focused(app):find('"View"', 1, true)).never.to_be_nil()
	app:key("ArrowLeft")
	app:frames(4)
	expect(focused(app):find('"File"', 1, true)).never.to_be_nil()
	app:key("ArrowDown")
	app:frames(20)
	expect(app:has('role:menuitem "New chat"')).to_be(true)
	-- Its shortcut as Linux and Windows write one.
	expect(app:tree('role:menu "File"')).to_contain('"Ctrl+N"')
	app:key("Escape")
	app:frames(20)
	app:key("Escape")
	app:frames(20)
	expect(app:has("#titlebar-menus")).to_be(false)
	expect(app:state("app.menu_alt")).to_be(false)
end)

test("Alt again, or a click elsewhere, turns them back", function()
	local app = open()
	app:command("fixture.alt")
	app:frames(20)
	app:command("fixture.alt")
	app:frames(20)
	expect(app:has("#titlebar-menus")).to_be(false)
	app:command("fixture.alt")
	app:frames(20)
	expect(app:has("#titlebar-menus")).to_be(true)
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(20)
	expect(app:has("#titlebar-menus")).to_be(false)
end)

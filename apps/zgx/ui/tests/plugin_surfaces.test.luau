-- Plugins' pages and tabs in the main window (ui/plugin_surfaces_fixture):
-- a listed page is a row under Search that opens it over the chats, its
-- placeholder there for the plugin to fill (`.plugin-page.plugin-<id>`); a
-- tab opens in the open session's panel, once however often it's asked,
-- its placeholder there (`.plugin-tab.plugin-<id>`).

local function open(): any
	local app = mount("plugin_surfaces_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	return app
end

test("a plugin's page is a row in the sidebar that opens it over the chats", function()
	local app = open()
	expect(app:has(".plugin-page-row")).to_be(false)
	app:command("page")
	app:frames(20)
	expect(app:has('.plugin-page-row >> "Pull requests"')).to_be(true)
	app:click(".plugin-page-row")
	app:frames(30)
	expect(app:has(".plugin-page.plugin-prs")).to_be(true)
	expect(app:tree()).to_contain("Its plugin isn't on right now.")
	-- A session again: the page goes, the chats as they were.
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(30)
	expect(app:has(".plugin-page.plugin-prs")).to_be(false)
end)

test("a plugin's tab opens in the session's panel, once", function()
	local app = open()
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(20)
	app:command("tab")
	app:frames(40)
	expect(app:wait('#panel-strip >> role:tab "PR 12"', { timeout_ms = 15000 })).to_be(true)
	expect(app:has(".plugin-tab.plugin-prs")).to_be(true)
	app:command("tab")
	app:frames(20)
	local n = 0
	for _ in app:tree("#panel-strip"):gmatch('"PR 12"') do
		n += 1
	end
	expect(n).to_be(1)
end)

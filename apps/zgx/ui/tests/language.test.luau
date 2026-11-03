-- Settings → General → Language: the window's words follow the language
-- chosen at once (no reload), it's kept as chosen, and "System" follows the
-- Mac's again. Settings search finds it by its Chinese name too. On the
-- shell in ui/shell_fixture.luau. (The words themselves: ui/locales, whose
-- keys `zgx lint` and src/i18n_tests.rs check.)

local function shows(app: any, scope: string, text: string): boolean
	return app:tree(scope):find(text, 1, true) ~= nil
end

local PAGE = "#settings-page"
local NAV = "#settings-nav"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	return app
end

test("Language follows the Mac's until one is chosen, and lists each language in its own words", function()
	local app = open()
	expect(app:state("settings_language")).to_be("system")
	expect(shows(app, PAGE, "Language")).to_be(true)
	expect(shows(app, PAGE, "Follows your Mac's preferred language.")).to_be(true)
	app:click(`{PAGE} >> role:button "Language"`)
	app:frames(10)
	expect(app:has('role:menuitem "English"')).to_be(true)
	expect(app:has('role:menuitem "简体中文"')).to_be(true)
end)

test("choosing 简体中文 changes the page's words at once, and English brings them back", function()
	local app = open()
	expect(shows(app, PAGE, "Send messages with")).to_be(true)
	app:click(`{PAGE} >> role:button "Language"`)
	app:frames(10)
	app:click('role:menuitem "简体中文"')
	app:frames(20)
	expect(app:state("settings_language")).to_be("zh-Hans")
	-- The same page, now in Chinese: none of its English is left.
	expect(app:state("shell_route_path")).to_be("/settings/general")
	expect(shows(app, PAGE, "Send messages with")).to_be(false)
	expect(shows(app, PAGE, "Compact mode")).to_be(false)
	expect(shows(app, PAGE, "简体中文")).to_be(true)
	-- English again, as chosen in the same select (its name now in Chinese:
	-- reached by its id).
	app:click("#settings-language >> role:button")
	app:frames(10)
	app:click('role:menuitem "English"')
	app:frames(20)
	expect(app:state("settings_language")).to_be("en")
	expect(shows(app, PAGE, "Send messages with")).to_be(true)
	expect(shows(app, PAGE, "Compact mode")).to_be(true)
end)

test("Settings search finds Language by its Chinese name, in English too", function()
	local app = open()
	app:type(`{NAV} >> #settings-search`, "语言")
	app:frames(10)
	expect(app:has(`#settings-search-results >> "Language"`)).to_be(true)
	app:key("Enter")
	app:frames(30)
	expect(app:state("settings_search_lit")).to_be("language")
end)

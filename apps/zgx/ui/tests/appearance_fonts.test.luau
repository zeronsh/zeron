-- Settings → Appearance's fonts: the interface's and code's, each picked
-- from this computer's (src/fonts.rs, the stub's four here) by a search,
-- code's listing monospaced ones alone; a pick is the window's font token at
-- once, kept across launches, and Default gives the theme's own back.
-- Settings on the shell in ui/shell_fixture.luau.

local PAGE = "#settings-page"
local NAV = "#settings-nav"

local function appearance(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Appearance"`)
	app:frames(30)
	return app
end

-- The window's font tokens as overridden now (nil: the theme's own).
local function fonts(app: any): (string?, string?)
	app:command("settings_stub.read_theme")
	app:frames(2)
	local t = app:state("settings_stub_window_theme") or {}
	return t.sans, t.mono
end

local function open(app: any, picker: string)
	-- (Below the page's fold: scrolled to first.)
	app:scroll(PAGE, 200)
	app:frames(10)
	app:click(`{PAGE} >> role:button "{picker}"`)
	app:frames(30)
end

test("Appearance: an interface font picked from this computer's is the window's at once, and kept", function()
	local app = appearance()
	expect(app:has(`{PAGE} >> "Interface font"`)).to_be(true)
	expect(app:has(`{PAGE} >> "Code font"`)).to_be(true)
	-- The theme's own until one is picked.
	local sans, mono = fonts(app)
	expect(sans).to_be(nil)
	expect(mono).to_be(nil)
	open(app, "Interface font")
	expect(app:has('role:dialog >> "Default (Geist)"')).to_be(true)
	-- Every family, proportional and monospaced alike.
	for _, family in { "Fira Code", "Inter", "JetBrains Mono", "Noto Serif" } do
		expect(app:has(`role:dialog >> "{family}"`)).to_be(true)
	end
	-- Searched by name.
	app:type('role:textbox "Search fonts"', "int")
	app:frames(20)
	expect(app:has('role:dialog >> "Inter"')).to_be(true)
	expect(app:has('role:dialog >> "Noto Serif"')).to_be(false)
	app:click('role:dialog >> "Inter"')
	app:frames(20)
	sans, mono = fonts(app)
	-- (Quoted: read as a family's name whatever it looks like.)
	expect(sans).to_be('"Inter"')
	expect(mono).to_be(nil)
	expect(app:state("interface_font")).to_be("Inter")
	-- (The trigger's name rolls over to it.)
	app:frames(40)
	expect(app:has(`{PAGE} >> role:button "Interface font" >> "Inter"`)).to_be(true)
	-- The next launch wears it from the start.
	local again = appearance()
	sans, mono = fonts(again)
	expect(sans).to_be('"Inter"')
	expect(mono).to_be(nil)
	expect(again:has(`{PAGE} >> role:button "Interface font" >> "Inter"`)).to_be(true)
end)

test("Appearance: the code font lists monospaced fonts alone and sets code's token", function()
	local app = appearance()
	open(app, "Code font")
	expect(app:has('role:dialog >> "Default (Geist Mono)"')).to_be(true)
	expect(app:has('role:dialog >> "Fira Code"')).to_be(true)
	expect(app:has('role:dialog >> "JetBrains Mono"')).to_be(true)
	expect(app:has('role:dialog >> "Inter"')).to_be(false)
	expect(app:has('role:dialog >> "Noto Serif"')).to_be(false)
	app:click('role:dialog >> "JetBrains Mono"')
	app:frames(20)
	local sans, mono = fonts(app)
	expect(sans).to_be(nil)
	expect(mono).to_be('"JetBrains Mono"')
	expect(app:state("code_font")).to_be("JetBrains Mono")
	-- Kept for the next launch.
	local again = appearance()
	sans, mono = fonts(again)
	expect(mono).to_be('"JetBrains Mono"')
end)

test("Appearance: Default gives each font back to the theme's own", function()
	local app = appearance()
	app:set_state("interface_font", "Inter")
	app:set_state("code_font", "Fira Code")
	app:frames(20)
	local sans, mono = fonts(app)
	expect(sans).to_be('"Inter"')
	expect(mono).to_be('"Fira Code"')
	open(app, "Interface font")
	app:click('role:dialog >> "Default (Geist)"')
	app:frames(20)
	open(app, "Code font")
	app:click('role:dialog >> "Default (Geist Mono)"')
	app:frames(20)
	sans, mono = fonts(app)
	expect(sans).to_be(nil)
	expect(mono).to_be(nil)
	expect(app:state("interface_font")).to_be("")
	expect(app:state("code_font")).to_be("")
end)

test("Appearance: a font chosen but no longer installed is still the one the picker says", function()
	local app = appearance()
	app:set_state("code_font", "Gone Mono")
	app:frames(60)
	expect(app:has(`{PAGE} >> role:button "Code font" >> "Gone Mono"`)).to_be(true)
	open(app, "Code font")
	expect(app:has('role:dialog >> "Gone Mono"')).to_be(true)
end)

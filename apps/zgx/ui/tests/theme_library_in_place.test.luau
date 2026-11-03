-- Your theme library (screens/settings/theme_library, src/themes.rs): an
-- imported VS Code theme is listed under "Imported" in the theme choices,
-- choosing it dresses the window, removing it gives Zeron's own back, and a
-- broken file says what's wrong in the shipping app's words. On the page
-- in ui/theme_library_fixture.luau.

local function open(): any
	local app = mount("theme_library_fixture.luau")
	app:resize(900, 900)
	app:frames(20)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

-- Opens a choice and picks `option`: imported themes come last, past the
-- list's fold, so it scrolls to the end first.
local function choose(app: any, select: string, option: string)
	app:click(`role:button "{select}"`)
	expect(app:wait(`role:menuitem "{option}"`, { timeout_ms = 15000 })).to_be(true)
	app:wait('"nothing"', { timeout_ms = 400 })
	app:scroll('role:menuitem "Zeron Dark"', 4000)
	app:frames(20)
	app:click(`role:menuitem "{option}"`)
	app:frames(20)
end

test("an imported theme is listed under Imported, and choosing it dresses the window", function()
	local app = open()
	expect(app:text("#applied")).to_be("none")
	app:command("import_harbor")
	app:frames(20)
	expect(calls(app)).to_contain("import:/themes/Harbor Night.json")
	-- Listed in the library, and among the dark choices, under its heading.
	expect(app:has('"Harbor Night"')).to_be(true)
	app:click('role:button "Dark theme"')
	expect(app:wait('role:menuitem "Harbor Night"', { timeout_ms = 15000 })).to_be(true)
	expect(app:has('"Imported"')).to_be(true)
	expect(app:has('"Built in"')).to_be(true)
	app:key("Escape")
	app:frames(20)
	choose(app, "Dark theme", "Harbor Night")
	expect(app:text("#applied")).to_be("#d8dee9")
end)

test("removing the theme in use gives Zeron's own back", function()
	local app = open()
	app:command("import_harbor")
	app:frames(20)
	choose(app, "Dark theme", "Harbor Night")
	expect(app:text("#applied")).to_be("#d8dee9")
	app:click('role:button "More for Harbor Night"')
	expect(app:wait('role:menuitem "Remove"', { timeout_ms = 15000 })).to_be(true)
	app:wait('"nothing"', { timeout_ms = 400 })
	app:click('role:menuitem "Remove"')
	app:frames(20)
	app:click('role:button "Remove"')
	app:frames(20)
	expect(calls(app)).to_contain("remove:custom-harbor-night")
	expect(app:state("palette_dark")).to_be("zeron-dark")
	expect(app:text("#applied")).to_be("none")
	-- (Its row in the library is gone; a closed menu's rows linger hidden.)
	expect(app:has('role:button "More for Harbor Night"')).to_be(false)
end)

test("renaming follows into the choices", function()
	local app = open()
	app:command("import_harbor")
	app:frames(20)
	app:click('role:button "More for Harbor Night"')
	expect(app:wait('role:menuitem "Rename"', { timeout_ms = 15000 })).to_be(true)
	app:wait('"nothing"', { timeout_ms = 400 })
	app:click('role:menuitem "Rename"')
	app:frames(20)
	app:fill('role:textbox "Name"', "Harbor")
	app:click('role:button "Rename"')
	app:frames(20)
	expect(calls(app)).to_contain("rename:custom-harbor-night:Harbor")
	expect(app:has('role:button "More for Harbor"')).to_be(true)
end)

test("a broken file says what's wrong and adds nothing", function()
	local app = open()
	app:command("import_broken")
	app:frames(20)
	expect(app:has('"This theme couldn\'t be imported"')).to_be(true)
	expect(app:tree()).to_contain("could not parse JSONC theme /themes/Broken.json")
	expect(app:has('role:button "More for Harbor Night"')).to_be(false)
	-- The next import clears it.
	app:command("import_harbor")
	app:frames(20)
	expect(app:has('"This theme couldn\'t be imported"')).to_be(false)
end)

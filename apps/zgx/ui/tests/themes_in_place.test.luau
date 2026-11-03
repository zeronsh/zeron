-- Settings → Themes (screens/themes): every theme Zeron can wear, apart
-- from the template it runs. Built in, a template's own and yours, each
-- under its tab with its swatch; Use wears a theme for each scheme it has
-- (one with only dark fills dark alone), and the menu wears one for a
-- single scheme; a scheme with nothing chosen wears the running template's
-- own theme, a chosen one wins over it; import by the button or a drop,
-- rename and remove; and Settings → Appearance's quick picks. On
-- ui/themes_fixture.luau.

local function open(page: string?): any
	local app = mount("themes_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:command(page or "themes")
	app:frames(20)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

-- The rows' ids, top to bottom.
local function order(app: any): { string }
	local out = {}
	for line in app:tree():gmatch("[^\n]+") do
		local id = line:match("#theme%-([%w%-]+) ")
		if id and id ~= "swatch" and not table.find(out, id) then
			table.insert(out, id)
		end
	end
	return out
end

local function tab(app: any, name: string)
	app:click(`role:tab "{name}"`)
	app:frames(20)
end

local function menu(app: any, theme: string, item: string)
	app:click(`role:button "More for {theme}"`)
	expect(app:wait(`role:menuitem "{item}"`, { timeout_ms = 15000 })).to_be(true)
	app:wait('"nothing"', { timeout_ms = 400 })
	app:click(`role:menuitem "{item}"`)
	app:frames(20)
end

local function read(app: any, what: string): string
	app:command("read")
	app:frames(5)
	return tostring(app:state(what))
end

test("every theme is listed, Zeron's first, each with its swatch, what it has and where it's from", function()
	local app = open()
	local rows = order(app)
	expect(rows[1]).to_be("zeron")
	expect(table.concat(rows, ",")).to_be("zeron,gruvbox,github,dracula,ayu-mirage,template-ledger")
	expect(app:has('#theme-zeron >> ~"The default, light and dark"')).to_be(true)
	expect(app:has('#theme-gruvbox >> ~"Light and dark"')).to_be(true)
	expect(app:has('#theme-dracula >> ~"Dark only"')).to_be(true)
	expect(app:has('#theme-gruvbox >> ~"Built in"')).to_be(true)
	expect(app:has('#theme-template-ledger >> ~"From Ledger"')).to_be(true)
	for _, id in rows do
		expect(app:has(`#theme-{id} >> .theme-swatch`)).to_be(true)
	end
	-- Each half wears its scheme's palette: Gruvbox both, Dracula dark alone.
	expect(app:count("#theme-gruvbox >> .theme-swatch-half")).to_be(2)
	expect(app:count("#theme-dracula >> .theme-swatch-half")).to_be(1)
	-- Nothing chosen, the default template: Zeron's is in use.
	expect(app:has('#theme-zeron >> .theme-in-use >> "In use"')).to_be(true)
	expect(app:has('#theme-gruvbox >> role:button "Use"')).to_be(true)
end)

test("the tabs hold what they say, with their counts, and the search finds by name", function()
	local app = open()
	expect(app:has('role:tab "All" >> "6\"')).to_be(true)
	expect(app:has('role:tab "Built in" >> "5\"')).to_be(true)
	tab(app, "From templates")
	expect(table.concat(order(app), ",")).to_be("template-ledger")
	tab(app, "Yours")
	expect(app:has('~"None of yours yet"')).to_be(true)
	expect(app:has('role:button "Import theme"')).to_be(true)
	tab(app, "All")
	app:type('role:textbox "Search themes"', "grUV")
	app:frames(20)
	expect(table.concat(order(app), ",")).to_be("gruvbox")
	app:fill('role:textbox "Search themes"', "ledger")
	app:frames(20)
	expect(table.concat(order(app), ",")).to_be("template-ledger")
end)

test("Use wears a theme for each scheme it has; one with only dark fills dark alone", function()
	local app = open()
	app:click('#theme-gruvbox >> role:button "Use"')
	app:frames(20)
	expect(calls(app)).to_be("choose:light:gruvbox-light,choose:dark:gruvbox-dark")
	expect(app:has('#theme-gruvbox >> .theme-in-use >> "In use"')).to_be(true)
	expect(app:has('#theme-gruvbox >> role:button "Use"')).to_be(false)
	expect(app:has('#theme-zeron >> role:button "Use"')).to_be(true)
	app:click('#theme-dracula >> role:button "Use"')
	app:frames(20)
	expect(calls(app)).to_contain("choose:dark:dracula")
	expect(calls(app)).never.to_contain("choose:light:dracula")
	-- Following the system: Gruvbox for light, Dracula for dark.
	expect(app:has('#theme-gruvbox >> .theme-in-use >> "Light"')).to_be(true)
	expect(app:has('#theme-dracula >> .theme-in-use >> "Dark"')).to_be(true)
	expect(app:has('#theme-dracula >> role:button "Use"')).to_be(false)
	expect(app:has('#theme-gruvbox >> role:button "Use"')).to_be(true)
end)

test("the menu wears a theme for one scheme alone", function()
	local app = open()
	menu(app, "GitHub", "Use only for dark")
	expect(calls(app)).to_be("choose:dark:github-dark")
	expect(app:has('#theme-github >> .theme-in-use >> "Dark"')).to_be(true)
	expect(app:has('#theme-zeron >> .theme-in-use >> "Light"')).to_be(true)
	-- Only a theme with both has those; built in, none of yours' items.
	app:click('role:button "More for GitHub"')
	expect(app:wait('role:menuitem "Use only for light"', { timeout_ms = 15000 })).to_be(true)
	expect(app:has('role:menuitem "Rename"')).to_be(false)
	expect(app:has('role:menuitem "Remove"')).to_be(false)
	app:key("Escape")
	app:frames(10)
	expect(app:has('role:button "More for Dracula"')).to_be(false)
end)

test("with a fixed scheme one theme shows, and using a dark one in light mode shows dark", function()
	local app = open()
	app:command("scheme_light")
	app:frames(20)
	app:click('#theme-gruvbox >> role:button "Use"')
	app:frames(20)
	expect(app:has('#theme-gruvbox >> .theme-in-use >> "In use"')).to_be(true)
	app:click('#theme-dracula >> role:button "Use"')
	app:frames(20)
	expect(read(app, "worn")).to_contain("theme-dark")
	expect(app:has('#theme-dracula >> .theme-in-use >> "In use"')).to_be(true)
	-- In dark now, Gruvbox's light is kept but doesn't show.
	expect(app:has('#theme-gruvbox >> .theme-in-use >> "In use"')).to_be(false)
end)

test("the window wears what's chosen, scheme by scheme", function()
	local app = open()
	app:command("scheme_light")
	app:frames(20)
	-- (Zeron's own colours, whatever the glass makes them.)
	local own_light = read(app, "worn")
	expect(own_light).to_contain("theme-light ")
	app:command("scheme_dark")
	app:frames(20)
	local own_dark = read(app, "worn")
	app:command("scheme_light")
	app:frames(20)
	app:click('#theme-gruvbox >> role:button "Use"')
	app:frames(20)
	expect(read(app, "worn")).to_contain("theme-light #fbf1c7")
	app:command("scheme_dark")
	app:frames(20)
	expect(read(app, "worn")).to_contain("theme-dark #282828")
	-- Back to Zeron's: its own colours again.
	app:click('#theme-zeron >> role:button "Use"')
	app:frames(20)
	expect(read(app, "worn")).to_be(own_dark)
	app:command("scheme_light")
	app:frames(20)
	expect(read(app, "worn")).to_be(own_light)
end)

test("a template's own theme is worn where nothing is chosen, and a chosen theme wins over it", function()
	local app = open()
	-- Ledger runs, bringing a dark theme: nothing chosen, dark wears it.
	app:command("use_ledger")
	app:frames(20)
	expect(read(app, "slots")).to_be("zeron ledger")
	expect(app:has('#theme-template-ledger >> .theme-in-use >> "Dark"')).to_be(true)
	expect(app:has('#theme-zeron >> .theme-in-use >> "Light"')).to_be(true)
	-- Gruvbox chosen: the template stays, its theme gives way for both.
	app:click('#theme-gruvbox >> role:button "Use"')
	app:frames(20)
	expect(read(app, "slots")).to_be("gruvbox-light gruvbox-dark")
	expect(app:has('#theme-gruvbox >> .theme-in-use >> "In use"')).to_be(true)
	expect(app:has('#theme-template-ledger >> .theme-in-use >> "Dark"')).to_be(false)
	-- And a template's theme is one to use as any other is.
	app:click('#theme-template-ledger >> role:button "Use"')
	app:frames(20)
	expect(calls(app)).to_contain("choose:dark:template-ledger-dark")
	expect(read(app, "slots")).to_be("gruvbox-light template-ledger-dark")
	app:click('role:button "More for Ledger Ink"')
	expect(app:wait('role:menuitem "Show in Templates"', { timeout_ms = 15000 })).to_be(true)
end)

test("Import theme brings a VS Code theme in, under Yours, and a broken file says why", function()
	local app = open()
	app:copy_file("tests/fixtures/themes/Harbor Night.json", "Harbor Night.json")
	app:pick({ "Harbor Night.json" })
	app:click('role:button "Import theme"')
	app:frames(30)
	expect(calls(app)).to_contain("import:Harbor Night.json")
	expect(app:has('role:tab "Yours" >> "1\"')).to_be(true)
	expect(app:has('#theme-custom-harbor-night >> ~"Imported"')).to_be(true)
	expect(app:has('#theme-custom-harbor-night >> ~"Dark only"')).to_be(true)
	app:copy_file("tests/fixtures/themes/Broken.json", "Broken.json")
	app:pick({ "Broken.json" })
	app:click('role:button "Import theme"')
	app:frames(30)
	expect(app:has('~"This theme couldn\'t be imported"')).to_be(true)
	expect(app:has('~"could not parse JSONC theme"')).to_be(true)
end)

test("a theme dropped on the page is imported", function()
	local app = open()
	expect(app:has("#themes-page")).to_be(true)
	app:command("drop_harbor")
	app:frames(30)
	expect(calls(app)).to_contain("import:Harbor Night.json")
	expect(app:has("#theme-custom-harbor-night")).to_be(true)
end)

test("yours can be renamed, and removing the one in use gives its scheme back", function()
	local app = open()
	app:command("drop_harbor")
	app:frames(30)
	app:click('#theme-custom-harbor-night >> role:button "Use"')
	app:frames(20)
	expect(app:has('#theme-custom-harbor-night >> .theme-in-use >> "Dark"')).to_be(true)
	menu(app, "Harbor Night", "Rename")
	app:fill('role:textbox "Name"', "Harbor")
	app:click('role:button "Rename"')
	app:frames(20)
	expect(calls(app)).to_contain("rename:custom-harbor-night:Harbor")
	expect(app:has('#theme-custom-harbor-night >> ~"Harbor"')).to_be(true)
	menu(app, "Harbor", "Remove")
	expect(app:has('~"Remove this theme?"')).to_be(true)
	app:click('role:button "Remove"')
	app:frames(20)
	expect(calls(app)).to_contain("remove:custom-harbor-night")
	expect(app:has("#theme-custom-harbor-night")).to_be(false)
	expect(app:has('~"None of yours yet"')).to_be(true)
	tab(app, "All")
	expect(app:has('#theme-zeron >> .theme-in-use >> "In use"')).to_be(true)
end)

test("a change made outside the window shows on the page and restyles the theme in use", function()
	local app = open()
	app:command("drop_harbor")
	app:frames(30)
	app:command("scheme_dark")
	app:frames(20)
	app:click('#theme-custom-harbor-night >> role:button "Use"')
	app:frames(20)
	expect(read(app, "worn")).to_contain("theme-dark #14181f")
	-- An agent changes Harbor Night's colours and makes another theme.
	app:command("agent_edits")
	app:frames(30)
	expect(read(app, "worn")).to_contain("theme-dark #2a1430")
	expect(app:has("#theme-custom-ink")).to_be(true)
	expect(app:has('#theme-custom-ink >> ~"Imported"')).to_be(true)
	expect(app:has('#theme-custom-harbor-night >> .theme-in-use >> "In use"')).to_be(true)
end)

test("Appearance picks a theme for each scheme showing, and leads to the Themes page", function()
	local app = open("appearance")
	-- Following the system: one for light, one for dark.
	expect(app:has('role:button "Light theme"')).to_be(true)
	expect(app:has('role:button "Dark theme"')).to_be(true)
	app:click('role:button "Dark theme"')
	expect(app:wait('role:menuitem "Dracula"', { timeout_ms = 15000 })).to_be(true)
	-- Only dark ones, grouped as the page's tabs are.
	expect(app:has('role:menuitem "Ayu Mirage"')).to_be(true)
	expect(app:has('role:menuitem "Ledger Ink"')).to_be(true)
	expect(app:has('"From templates"')).to_be(true)
	app:wait('"nothing"', { timeout_ms = 400 })
	app:click('role:menuitem "Dracula"')
	app:frames(20)
	expect(calls(app)).to_be("choose:dark:dracula")
	-- A fixed scheme: one pick, called Theme.
	app:command("scheme_light")
	app:frames(20)
	expect(app:has('role:button "Theme"')).to_be(true)
	expect(app:has('role:button "Dark theme"')).to_be(false)
	-- All themes opens the page.
	app:click('~"All themes"')
	app:frames(30)
	expect(app:has("#themes-page")).to_be(true)
end)

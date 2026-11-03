-- Settings → Templates: Zeron's official templates (its built-in themes,
-- src/plugins/theme_templates.rs) are listed as Zeron's own is, right after
-- it: the same row, by Zeron, Use and the same ⋯ menu (no Delete, no
-- Export), previewed from their colours as every template is. Not yours,
-- and no tab of their own. On ui/templates_fixture.luau.

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

local function open(): any
	local app = mount("templates_fixture.luau")
	app:resize(1000, 900)
	app:frames(20)
	return app
end

-- The rows' ids, top to bottom.
local function order(app: any): { string }
	local out = {}
	for line in app:tree():gmatch("[^\n]+") do
		local id = line:match("#template%-([%w%-]+) ")
		if id and not table.find(out, id) then
			table.insert(out, id)
		end
	end
	return out
end

test("official templates list after Zeron's own, by Zeron, previewed as every template is", function()
	local app = open()
	local rows = order(app)
	expect(rows[1]).to_be("zeron")
	expect(rows[2]).to_be("theme-tokyo-night")
	expect(rows[3]).to_be("theme-github")
	expect(app:has('#template-theme-tokyo-night >> ~"by Zeron"')).to_be(true)
	expect(app:has('#template-theme-tokyo-night >> ~"Tokyo Night\'s colours on Zeron\'s interface."')).to_be(true)
	expect(app:has("#template-theme-tokyo-night >> .template-swatch")).to_be(true)
	expect(app:has("#template-zeron >> .template-swatch")).to_be(true)
	expect(app:has('#template-theme-github >> role:button "Use"')).to_be(true)
	-- No tab of their own; not yours.
	expect(app:has('role:tab "Themes"')).to_be(false)
	app:click('role:tab "Yours"')
	app:frames(20)
	expect(app:has("#template-theme-github")).to_be(false)
end)

test("an official template's menu is Zeron's own: no Delete, no Export", function()
	local app = open()
	app:click('role:button "More for GitHub"')
	expect(app:wait('role:menuitem "Duplicate"', { timeout_ms = 15000 })).to_be(true)
	expect(app:has('role:menuitem "Show folder"')).to_be(true)
	expect(app:has('role:menuitem "Delete"')).to_be(false)
	expect(app:has('role:menuitem "Export…"')).to_be(false)
end)

test("using an official template puts it in use, and Zeron's own goes back", function()
	local app = open()
	app:click('#template-theme-github >> role:button "Use"')
	app:frames(20)
	expect(calls(app)).to_contain("use:theme-github")
	expect(app:has('#template-theme-github >> ~"In use"')).to_be(true)
	expect(app:has('#template-zeron >> role:button "Use"')).to_be(true)
	app:click('#template-zeron >> role:button "Use"')
	app:frames(20)
	expect(app:has('#template-theme-github >> role:button "Use"')).to_be(true)
end)

test("the search finds an official template by name", function()
	local app = open()
	app:type('role:textbox "Search templates"', "tokyo")
	app:frames(20)
	expect(app:has("#template-theme-tokyo-night")).to_be(true)
	expect(app:has("#template-theme-github")).to_be(false)
end)

-- Every template's preview is drawn from its colours: the themes each half
-- wears (state.previews), light then dark.
local function previews(app: any, id: string): string
	local all = app:state("templates_previews") or {}
	return table.concat(all[id] or {}, ",")
end

test("every template's preview is drawn from its colours: Zeron's own, both, only dark, none", function()
	local app = open()
	-- Zeron's own: its two themes.
	expect(previews(app, "zeron")).to_be("zeron-light,zeron-dark")
	-- A template of yours with both appearances: each its own.
	expect(previews(app, "hot-pink")).to_be("preview-hot-pink-light,preview-hot-pink-dark")
	-- Only dark: that half alone, across the whole card.
	expect(previews(app, "night")).to_be("preview-night-dark")
	-- No colour changed: Zeron's own.
	expect(previews(app, "airy")).to_be("zeron-light,zeron-dark")
	-- An official template: its palettes, light and dark.
	expect(previews(app, "theme-github")).to_be("preview-theme-github-light,preview-theme-github-dark")
	-- Each row draws one, the same way.
	for _, id in { "zeron", "theme-github", "hot-pink", "night", "airy" } do
		expect(app:has(`#template-{id} >> .template-swatch`)).to_be(true)
	end
	-- The previews' themes are registered for real.
	app:command("theme_names")
	app:frames(5)
	local names = tostring(app:state("theme_names"))
	expect(names).to_contain("preview-hot-pink-dark")
	expect(names).to_contain("preview-night-dark")
	expect(names).to_contain("preview-theme-github-light")
end)

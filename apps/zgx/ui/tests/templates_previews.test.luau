-- Settings → Templates: every row's preview is drawn from its template's
-- colours (src/plugins/looks.rs, ds/theme_swatch). On
-- ui/templates_fixture.luau.

local function open(): any
	local app = mount("templates_fixture.luau")
	app:resize(1000, 900)
	app:frames(20)
	return app
end

-- Every template's preview is drawn from its colours: the themes each half
-- wears (state.previews), light then dark.
local function previews(app: any, id: string): string
	local all = app:state("templates_previews") or {}
	return table.concat(all[id] or {}, ",")
end

test("every template's preview is drawn from its colours: the default's, both, only dark, none", function()
	local app = open()
	-- The default: its two themes.
	expect(previews(app, "zeron")).to_be("zeron-light,zeron-dark")
	-- A template of yours with both appearances: each its own.
	expect(previews(app, "hot-pink")).to_be("preview-hot-pink-light,preview-hot-pink-dark")
	-- Only dark: that half alone, across the whole card.
	expect(previews(app, "night")).to_be("preview-night-dark")
	-- No colour changed: the default's.
	expect(previews(app, "airy")).to_be("zeron-light,zeron-dark")
	-- Each row draws one, the same way.
	for _, id in { "zeron", "hot-pink", "night", "airy" } do
		expect(app:has(`#template-{id} >> .template-swatch`)).to_be(true)
	end
	-- The previews' themes are registered for real.
	app:command("theme_names")
	app:frames(5)
	local names = tostring(app:state("theme_names"))
	expect(names).to_contain("preview-hot-pink-dark")
	expect(names).to_contain("preview-night-dark")
end)

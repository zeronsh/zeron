-- A new chat's wallpaper (ds/wallpaper) under an effect, as its art changes
-- (ui/composer_place_fixture.luau). The art fades in inside its layer over
-- the layer leaving, and a crossfade always ends on the newest art (art
-- once came up blank here as a picture changed, or the theme with it, under
-- Scanlines: zgx now draws a cached layer again once a picture in it has
-- decoded, zgx 2e22d99d). Pixels aren't in a test's reach; this pins the
-- shape that draws them (and
-- src/plugins/examples_tests.rs's Parallax scenes test, that a plugin's
-- `ui.change` still reaches the place: no presence nested round it).

test("wallpaper: new art crossfades over the old and ends on the newest", function()
	local app = mount("composer_place_fixture.luau")
	app:resize(900, 360)
	app:set_state("test_place_effect", "scanlines")
	app:wait("#never-shown", { timeout_ms = 900 })
	expect(app:has("#wallpaper-layer >> #wall-plate-1-night")).to_be(true)
	-- A picture alone: the new fades in over the old, each in its layer.
	app:set_state("test_place_picture", "plate-2-bluehour")
	app:frames(2)
	expect(app:has("#wallpaper-layer >> #wall-plate-1-night")).to_be(true)
	expect(app:has("#wallpaper-layer >> #wall-plate-2-bluehour")).to_be(true)
	app:wait("#never-shown", { timeout_ms = 900 })
	expect(app:count("#wallpaper-layer")).to_be(1)
	expect(app:has("#wall-plate-1-night")).to_be(false)
	expect(app:has("#wallpaper-layer >> #wall-plate-2-bluehour")).to_be(true)
	-- The theme and the picture at once (what came up blank): one layer, on
	-- the newest art.
	app:set_state("test_place_dark", false)
	app:set_state("test_place_picture", "plate-3-sunrise")
	app:frames(1)
	app:set_state("test_place_picture", "plate-1-night")
	app:wait("#never-shown", { timeout_ms = 900 })
	expect(app:count("#wallpaper-layer")).to_be(1)
	expect(app:has("#wallpaper-layer >> #wall-plate-1-night")).to_be(true)
	expect(app:has("#wall-plate-3-sunrise")).to_be(false)
	expect(app:has("#wall-plate-2-bluehour")).to_be(false)
	-- No wallpaper: no layer.
	app:set_state("test_place_art", false)
	app:wait("#never-shown", { timeout_ms = 900 })
	expect(app:count("#wallpaper-layer")).to_be(0)
	expect(app:error()).to_be(nil)
end)

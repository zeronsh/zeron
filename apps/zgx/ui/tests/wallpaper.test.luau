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

-- Where the art's box stands in the window (ui/shell_fixture.luau's tree):
-- its left edge and width.
local function art_at(app: any): (number, number)
	for line in app:tree("#shell"):gmatch("[^\n]+") do
		if line:find(" #wallpaper-layer", 1, true) then
			local x, w = line:match(" (%-?%d+),%-?%d+ (%d+)x%d+")
			return tonumber(x) or 0, tonumber(w) or 0
		end
	end
	return 0, 0
end

test("wallpaper: a new chat's art holds still as the sidebar folds and the panel opens, and covers the window as it resizes", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 900)
	app:frames(30)
	app:set_state("shell_wallpaper_image", { path = "assets/art/plate-1-night.jpg", name = "night", w = 2100, h = 700, x = 0.5, y = 0.5, zoom = 1 })
	app:set_state("shell_wallpaper", "image")
	app:set_state("shell_wallpaper_effect", "dither")
	app:frames(30)
	local x, w = art_at(app)
	expect(w).to_be(1400)
	-- The sidebar folding, and the panel opening beside the card, part way
	-- and done: the card's edges move, the art doesn't (Wing's art moved
	-- with the card's right edge as the panel opened).
	app:command("shell.toggle_sidebar")
	app:frames(4)
	expect({ art_at(app) }).to_equal({ x, w })
	app:frames(30)
	app:command("shell.toggle_files")
	app:frames(4)
	expect({ art_at(app) }).to_equal({ x, w })
	app:frames(30)
	expect({ art_at(app) }).to_equal({ x, w })
	-- The window narrower: the art is laid at its width, from the same edge.
	app:resize(1100, 800)
	app:frames(30)
	local nx, nw = art_at(app)
	expect(nw).to_be(1100)
	expect(nx).to_be(x)
	expect(app:error()).to_be_nil()
end)

-- The art's layer as the shell's tree shows it: its node and whether it's
-- hidden ("" when there's none).
local function layer(app: any): (string, boolean)
	for line in app:tree("#shell"):gmatch("[^\n]+") do
		if line:find(" #wallpaper-layer", 1, true) then
			return line:match("@(%d+)") or "", line:find(" hidden", 1, true) ~= nil
		end
	end
	return "", false
end

test("wallpaper: a new chat's picture stays built while a session shows, and is back at once", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 900)
	app:frames(30)
	app:set_state("shell_wallpaper_image", { path = "assets/art/plate-1-night.jpg", name = "night", w = 2100, h = 700, x = 0.5, y = 0.5, zoom = 1 })
	app:set_state("shell_wallpaper", "image")
	app:set_state("shell_wallpaper_effect", "dither")
	app:frames(30)
	local node, hidden = layer(app)
	expect(node).never.to_be("")
	expect(hidden).to_be(false)
	-- A session: the art is hidden, not torn down (it was, and a new chat
	-- then decoded it and faded it in anew, a third of a second late).
	app:command("shell.session_1")
	app:frames(30)
	expect(app:state("shell_route_path")).never.to_be("/")
	expect({ layer(app) }).to_equal({ node, true })
	-- A new chat again: the same art, showing from the first frames.
	app:command("shell.new_chat")
	app:frames(2)
	expect({ layer(app) }).to_equal({ node, false })
	expect(app:count("#wallpaper-layer")).to_be(1)
	expect(app:error()).to_be_nil()
end)

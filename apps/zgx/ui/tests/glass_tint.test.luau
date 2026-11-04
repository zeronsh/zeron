-- The window's glass over a desktop that blurs less than macOS's 60 points
-- (ds/material and ds/glass_tint, `window.blur` from src/desktop_blur.rs: Hyprland's blur in
-- points, nil where the app can't tell): the tint keeps less of the theme's
-- see-through, so what is behind shows as colour, not as a sharp picture.

local glass_tint = require("@ds/glass_tint")

test("Glass tint: the theme's own over a macOS-like blur or where unknown, more opaque over less", function()
	expect(glass_tint.clarity(nil)).to_be(1)
	expect(glass_tint.clarity(60)).to_be(1)
	expect(glass_tint.clarity(40)).to_be(1)
	-- Hyprland's blur off: a quarter of the see-through is left.
	expect(glass_tint.clarity(0)).to_be(0.25)
	-- Omarchy's Glassy blur (size 4, passes 2: about 7 points at 1.25×).
	local glassy = glass_tint.clarity(7)
	expect(glassy > 0.25 and glassy < 0.5).to_be(true)
	expect(glass_tint.clarity(20) < glass_tint.clarity(30)).to_be(true)
end)

test("Glass tint: a tint keeps that much of its see-through", function()
	-- Zeron dark's (0.8 opaque) with none of its blur: 0.95.
	expect(glass_tint.thicker("#0b0b0dcc", 0.25)).to_be("#0b0b0df2")
	expect(glass_tint.thicker("#0b0b0dcc", 1)).to_be("#0b0b0dcc")
	expect(glass_tint.thicker("#0b0b0d", 0.25)).to_be_nil()
	expect(glass_tint.thicker("#0b0b0dff", 0.25)).to_be("#0b0b0dff")
end)

test("Glass tint: the shell wears it as the blur behind it changes", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	for _, blur in { 0, 7, 70, 0 } do
		app:set_state("window.blur", blur)
		app:frames(4)
		expect(app:error()).to_be_nil()
		expect(app:state("window.blur")).to_be(blur)
		expect(app:has("#shell")).to_be(true)
	end
end)

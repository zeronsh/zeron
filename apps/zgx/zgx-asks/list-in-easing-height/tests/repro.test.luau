test("a virtual list in a box easing shut and open lays out (panics today)", function()
	local app = mount("app.luau")
	app:resize(1000, 900)
	app:frames(5)
	-- Shut: each frame lays the list out in a shorter box. zgx f6506b6e6
	-- panics here: "layout allocation did not settle after 64 passes".
	app:set_state("open", false)
	for _ = 1, 30 do
		app:frames(1)
	end
	app:set_state("open", true)
	for _ = 1, 30 do
		app:frames(1)
	end
	expect(app:error()).to_be_nil()
	expect(app:has('"Row 1"')).to_be(true)
end)

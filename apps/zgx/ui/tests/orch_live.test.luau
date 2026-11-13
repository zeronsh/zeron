-- The orchestration live pieces (ui/orch_live_fixture.luau): a status
-- shows 300 ms at least before the next takes its place (ds/orch_hold),
-- the latest winning; cells and a sparkline draw and resize.

test("orch live: a status that changes at once is held, then the latest shows", function()
	local app = mount("orch_live_fixture.luau")
	app:resize(600, 400)
	app:frames(1)
	expect(app:text("#held")).to_be("working")
	-- Two changes inside the hold: neither shows yet.
	app:set_state("kind", "failed")
	app:frames(1)
	app:set_state("kind", "done")
	app:frames(1)
	expect(app:text("#held")).to_be("working")
	-- Past it: the latest.
	expect(app:wait("#held", { text = "done", timeout_ms = 2000 })).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch live: a change back to what shows drops the one waiting", function()
	local app = mount("orch_live_fixture.luau")
	app:resize(600, 400)
	app:frames(1)
	app:set_state("kind", "failed")
	app:frames(1)
	app:set_state("kind", "working")
	app:frames(40)
	expect(app:text("#held")).to_be("working")
end)

test("orch live: cells and a sparkline draw and resize", function()
	local app = mount("orch_live_fixture.luau")
	for _, size in { { 600, 400 }, { 320, 300 }, { 1200, 600 } } do
		app:resize(size[1], size[2])
		app:set_state("done", size[1] % 9)
		app:frames(5)
		expect(app:has("#cells")).to_be(true)
		expect(app:has("#spark")).to_be(true)
		expect(app:error()).to_be_nil()
	end
end)

-- The working ring and the status dots keep one clock: a loop made as one
-- comes into sight (or back into it, after a collapsed sidebar or a scroll
-- had it stopped) starts where `ui.now()` has the cycle, not from its start,
-- so every ring on screen turns together.

local function fixture(): any
	local app = mount("ring_visibility_fixture.luau")
	app:resize(800, 500)
	app:frames(3)
	expect(app:error()).to_be_nil()
	return app
end

test("a ring shown at any moment stands where the clock has its cycle, and one shown later as much later", function()
	local p = fixture():state("ring_phase")
	expect(p.first >= 0 and p.first < p.period).to_be(true)
	expect(p.later >= 0 and p.later < p.period).to_be(true)
	-- The later one stands later by the time between, no more (not back at
	-- its start, as a loop started afresh would).
	local apart = (p.later - p.first) % p.period
	expect(apart <= p.between + 1).to_be(true)
	-- And where the clock is now, give or take that time.
	local off = math.abs(p.now - p.later) % p.period
	expect(math.min(off, p.period - off) <= p.between + 50).to_be(true)
end)

test("the ring fixture shows each case (in the open, collapsed, scrolled away, outside, none) without an error", function()
	local app = fixture()
	for _, case in { "shown", "collapsed", "scrolled", "outside", "none" } do
		app:set_state("ring_case", case)
		app:frames(3)
		if case == "scrolled" then
			app:scroll("#scrolled", 2000)
			app:frames(3)
		end
		expect(app:error()).to_be_nil()
	end
end)

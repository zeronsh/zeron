-- An agent's presence is its light sweep: a soft band crosses the mark (and
-- a sub-agent's dot glyph) diagonal by diagonal and rests; it moves twelve
-- times a second, on one beat with the dots' pulse (one timer, no animation
-- playing); waiting and reduced motion stand still.

local sweep = require("@ds/sweep")

test("the band crosses the mark in a second and the dot quicker, from its first diagonal to its last, then rests", function()
	expect(sweep.BEAT).to_be(1000 / 12)
	expect(sweep.GLYPHS.agent.beats * sweep.BEAT).to_be(1000)
	expect(sweep.GLYPHS.subagent.beats < sweep.GLYPHS.agent.beats).to_be(true)
	for who, g in sweep.GLYPHS do
		local moving = g.beats - g.rest
		-- In at the first diagonal, out at the last, always onward.
		expect(sweep.band(who, 0)).to_be(-1)
		expect(sweep.band(who, moving - 1)).to_be(g.last + 1)
		for i = 1, moving - 1 do
			local step = sweep.band(who, i) - sweep.band(who, i - 1)
			expect(step > 0 and step <= 1).to_be(true)
		end
		-- Then off the glyph, and round again.
		for i = moving, g.beats - 1 do
			expect(sweep.band(who, i) < -2).to_be(true)
		end
		expect(sweep.band(who, g.beats)).to_be(-1)
	end
end)

test("working moves on the beat, connecting on every other, waiting, a held place and reduced motion stand still", function()
	local still, working = sweep.plan { who = "agent", pace = "working" }
	expect(still).to_be_nil()
	expect(working).to_be(1)
	local _, connecting = sweep.plan { who = "subagent", pace = "connecting" }
	expect(connecting).to_be(2)
	local off, none = sweep.plan { pace = "still" }
	expect(none).to_be_nil()
	expect(off < -2).to_be(true)
	local mid, reduced = sweep.plan { who = "agent", reduced = true }
	expect(reduced).to_be_nil()
	expect(mid).to_be(sweep.GLYPHS.agent.still)
	local held = sweep.plan { who = "subagent", at = 2 }
	expect(held).to_be(2)
end)

test("rows of working agents, a sub-agent, connecting and waiting draw, as rings too, and with motion reduced", function()
	local app = mount("presence_fixture.luau")
	app:resize(600, 600)
	app:frames(20)
	expect(app:error()).to_be_nil()
	expect(app:has('"Agent 12 at work"')).to_be(true)
	app:set_state("presence_glyph", "ring")
	app:frames(5)
	expect(app:error()).to_be_nil()
	app:set_state("ds/motion.luau::motion_reduce", true)
	app:set_state("presence_glyph", "sweep")
	app:frames(5)
	expect(app:error()).to_be_nil()
end)

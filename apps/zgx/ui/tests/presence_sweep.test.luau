-- An agent's presence is its light sweep: a soft band crosses the mark (and
-- a sub-agent's dot glyph) diagonal by diagonal and rests. One sweep where
-- you look (a chat's foot, two of its sub-agents); every other agent at work
-- sweeps a moment as it's drawn, then holds a still mark; waiting and
-- reduced motion stand still.

local sweep = require("@ds/sweep")

test("the band crosses the mark in a second and the dot quicker, from its first diagonal to its last, then rests", function()
	expect(sweep.GLYPHS.agent.beats * 1000 / 12).to_be(1000)
	expect(sweep.glyph("agent", 16)).to_be("agent")
	expect(sweep.glyph("agent", 12)).to_be("small")
	expect(sweep.glyph("subagent", 12)).to_be("subagent")
	expect(sweep.glyph(nil, 8)).to_be("micro")
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

test("one sweep where you look: rows sweep as they're drawn, then hold still; the foot and two sub-agents keep sweeping", function()
	local app = mount("presence_fixture.luau")
	app:resize(600, 800)
	app:frames(10)
	expect(app:error()).to_be_nil()
	local function moving(): number
		app:command("presence_count")
		app:frames(1)
		return app:state("presence_moving")
	end
	-- Drawn: 12 rows, connecting and the badge's micro mark start quiet; the
	-- foot and two of the three sub-agents sweep.
	expect(moving()).to_be(12 + 2 + 1 + 2)
	-- Past the start: only where you look.
	app:frames(200)
	expect(moving()).to_be(3)
	-- Reduced motion: nothing moves.
	app:set_state("ds/motion.luau::motion_reduce", true)
	app:set_state("presence_primary", false)
	app:set_state("presence_primary", true)
	app:frames(10)
	expect(moving()).to_be(0)
	expect(app:error()).to_be_nil()
end)

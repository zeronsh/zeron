-- An agent's presence is its light sweep: the band crosses every tile of the
-- mark (and of a sub-agent's dot glyph) and rests; it steps a sixth of a
-- second at a time, on one beat with the dots' pulse (one timer, no
-- animation playing); waiting and reduced motion stand still.

local sweep = require("@ds/sweep")

local function has(mask: number, bit: number): boolean
	return (mask // 2 ^ bit) % 2 == 1
end

test("the light sweep crosses every tile of the mark and of the dot, one band a step, then rests", function()
	for who, g in sweep.GLYPHS do
		expect(#g.steps).to_be(7)
		local seen = 0
		for i, lit in g.steps do
			-- Only its own tiles, some each step; the mark's band moves every step.
			expect(lit > 0).to_be(true)
			for bit = 0, 14 do
				if has(lit, bit) then
					expect(has(g.shape, bit)).to_be(true)
					if not has(seen, bit) then
						seen += 2 ^ bit
					end
				end
			end
			if who == "agent" and i > 1 then
				expect(lit ~= g.steps[i - 1]).to_be(true)
			end
		end
		expect(seen).to_be(g.shape)
	end
end)

test("working steps on the beat, connecting on every other, waiting, a held step and reduced motion stand still", function()
	expect(sweep.STEP).to_be(1000 / 6)
	local _, working = sweep.plan { who = "agent", pace = "working" }
	expect(working).to_be(1)
	local _, connecting = sweep.plan { who = "subagent", pace = "connecting" }
	expect(connecting).to_be(2)
	local rest, still = sweep.plan { pace = "still" }
	expect(still).to_be_nil()
	expect(rest[1]).to_be(0)
	local mid, reduced = sweep.plan { who = "agent", reduced = true }
	expect(reduced).to_be_nil()
	expect(mid[1] > 0).to_be(true)
	local _, held = sweep.plan { who = "subagent", at = 2 }
	expect(held).to_be_nil()
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

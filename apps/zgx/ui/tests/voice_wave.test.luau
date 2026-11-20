-- The voice bar's waveform follows the microphone (ds/voice_wave): a bar a
-- slot from the host's levels (`dictation_levels`, newest first), the newest
-- at the trailing edge, a dot where there's no slot yet, in the old app's
-- proportions (3 point bars, 3 point gaps, 16 high, the quietest 0.08 of it).

-- Bar `i`'s box from the tree: x, width, height.
local function bar(app: any, i: number): (number, number, number)
	local line = string.match(app:tree("#voice-wave"), `#voice%-bar%-{i} [^\n]*`)
	expect(line ~= nil).to_be(true)
	local x, w, h = string.match(line, "(%-?[%d.]+),%-?[%d.]+ ([%d.]+)x([%d.]+)")
	return tonumber(x) :: number, tonumber(w) :: number, tonumber(h) :: number
end

local function listening(): any
	local app = mount("dictation_fixture.luau")
	app:resize(960, 700)
	app:frames(30)
	app:command("listen")
	app:frames(30)
	return app
end

test("the bars follow the host's levels, newest at the trailing edge", function()
	local app = listening()
	-- No slots yet: dots on the baseline.
	local _, w, h = bar(app, 1)
	expect(w).to_be(3)
	expect(h).to_be(3)
	-- Levels 1, 0.5 and 0, newest first.
	app:command("levels")
	app:frames(3)
	local x1, _, h1 = bar(app, 1)
	local x2, _, h2 = bar(app, 2)
	local x3, _, h3 = bar(app, 3)
	expect(h1).to_be(16)
	-- 0.08 + 0.92 × 0.5 of 16 points.
	expect(math.abs(h2 - 8.64) < 0.5).to_be(true)
	expect(h3).to_be(3)
	-- One slot a bar, 6 points apart, the newest last.
	expect(x1 - x2).to_be(6)
	expect(x2 - x3).to_be(6)
	expect(app:error()).to_be(nil)
end)

test("finalizing holds the bars; starting is the baseline alone", function()
	local app = listening()
	app:command("levels")
	app:frames(3)
	app:command("finalizing")
	app:frames(5)
	local _, _, h = bar(app, 1)
	expect(h).to_be(16)
	app:command("starting")
	app:frames(5)
	expect(app:has("#voice-wave")).to_be(true)
	expect(app:has("#voice-bar-1")).to_be(false)
	expect(app:error()).to_be(nil)
end)

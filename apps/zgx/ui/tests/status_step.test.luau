-- Working rings and pulsing dots off macOS (ds/status): they step on one
-- shared clock, a timer that sleeps between steps, instead of 30 fps anime
-- loops (each kept zgx's Linux event loop turning between its frames, half
-- a core for one ring on screen). The clock runs only while one shows.

local STEP = "ds/status.luau::ds_status_step"

test("Status step: a ring and a dot on screen step the shared clock", function()
	local app = mount("status_step_fixture.luau")
	app:frames(5)
	expect(app:has("#status-step-row")).to_be(true)
	local seen: { [number]: boolean } = {}
	local n = 0
	for _ = 1, 10 do
		app:frames(6) -- 100 ms
		local t = app:state(STEP)
		if not seen[t] then
			seen[t] = true
			n += 1
		end
	end
	-- About one step a 100 ms: it moves, but nowhere near every frame.
	expect(n >= 5).to_be(true)
	expect(n <= 11).to_be(true)
end)

test("Status step: with none showing, the clock stops", function()
	local app = mount("status_step_fixture.luau")
	app:frames(12)
	app:set_state("status_step_shown", false)
	app:frames(12)
	local t = app:state(STEP)
	app:frames(60)
	expect(app:state(STEP)).to_be(t)
	-- Shown again, it steps again.
	app:set_state("status_step_shown", true)
	app:frames(30)
	expect(app:state(STEP) ~= t).to_be(true)
end)

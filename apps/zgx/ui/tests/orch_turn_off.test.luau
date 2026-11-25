-- Turning Orchestrators off while the Control room and every part of
-- account/orchestration are live: the window sets the shared value to nil
-- (src/orchestrators.rs), and nothing reading it fails. (Each part read on
-- the fixture's page, every tab built once.)

local TABS = { "board", "map", "chat", "notes" }

for _, shown in TABS do
	test(`turned off on the {shown} tab, its parts live: no error`, function()
		local app = mount("orch_turn_off_fixture.luau")
		app:resize(1440, 1000)
		-- Each tab built once (the Q&A's entries on Notes among them).
		for _, tab in TABS do
			app:set_state("control_tab", tab)
			app:frames(20)
		end
		app:set_state("control_tab", shown)
		app:frames(20)
		expect(app:error()).to_be_nil()
		expect(app:text("#orch-parts")).never.to_be("0 0 0 4")
		app:set_state("orchestration", nil)
		app:frames(30)
		expect(app:error()).to_be_nil()
		expect(app:text("#orch-parts")).to_be("0 0 0 4")
		-- And the switch off with it, as the window turns it off.
		app:set_state("plugins.on", { orchestrators = false })
		app:frames(30)
		expect(app:error()).to_be_nil()
	end)
end

-- Turned off while the Board moves a card (ds/orch_board_motion): in its
-- hold (800 ms, 48 frames) or its travel, the value gone nil, or the
-- switch off by itself; nothing fails, then or as the move's timers come
-- due. (Where the board has no motion, the move just lands.)
local CASES = {
	{ "the value nil in its hold", 2, true },
	{ "the value nil in its travel", 52, true },
	{ "the switch off in its hold", 2, false },
}
for _, case in CASES do
	local label, wait, value_first = case[1], case[2], case[3]
	test(`turned off mid-motion, {label}: no error`, function()
		local app = mount("orch_turn_off_fixture.luau")
		app:resize(1440, 1000)
		app:set_state("control_tab", "board")
		app:frames(30)
		app:command("fixture.card_row")
		app:frames(30)
		expect(app:error()).to_be_nil()
		app:command("fixture.card_move")
		app:frames(wait)
		expect(app:error()).to_be_nil()
		-- (Its flight, where the board has motion: no such state without.)
		local has, m = pcall(function(): any
			return app:state("orch_board_motion")
		end)
		if has and m ~= nil then
			expect(m.flying).to_be(1)
			expect((m.phase or {})["it-engine"]).to_be(if wait < 48 then "hold" else "travel")
		end
		if value_first then
			app:set_state("orchestration", nil)
			app:frames(2)
			expect(app:error()).to_be_nil()
		end
		app:set_state("plugins.on", { orchestrators = false })
		-- Past the hold and the travel (1150 ms), and then some.
		app:frames(120)
		expect(app:error()).to_be_nil()
	end)
end

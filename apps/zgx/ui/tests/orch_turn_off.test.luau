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

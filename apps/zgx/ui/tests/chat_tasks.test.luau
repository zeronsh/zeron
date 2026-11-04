-- A chat's background processes, as Zeron's tray has them
-- (background_tray.rs): Stop asks the chat's host to stop one, and says so
-- if it couldn't.

test("Stop on a background process asks its host, and says when it couldn't", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:command("task_running")
	app:frames(30)
	-- The tray stays folded to its header until opened.
	expect(app:has('role:button "Open Running"')).to_be(true)
	expect(app:has('role:button "Fold Running"')).to_be(false)
	app:click('role:button "Open Running"')
	app:frames(30)
	app:click('role:button "Stop npm run dev"')
	app:frames(10)
	expect(app:state("calls")).to_contain("stop_task:s2:bg1")
	expect(app:has('~"Couldn\'t stop that process"')).to_be(false)
	app:command("stop_fails")
	app:click('role:button "Stop npm run dev"')
	app:frames(10)
	expect(app:has('~"Couldn\'t stop that process"')).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("a background process outlives the turn: its tray stays as the turn ends", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:command("start_reply")
	app:command("task_running")
	app:frames(30)
	expect(app:has('role:button "Open Running"')).to_be(true)
	app:command("end_reply")
	-- Through the turn's end, every frame: the tray is there.
	for _ = 1, 30 do
		app:frames(1)
		expect(app:has('role:button "Open Running"')).to_be(true)
	end
	-- And still stoppable.
	app:click('role:button "Open Running"')
	app:frames(30)
	app:click('role:button "Stop npm run dev"')
	app:frames(10)
	expect(app:state("calls")).to_contain("stop_task:s2:bg1")
	expect(app:error()).to_be(nil)
end)

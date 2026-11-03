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

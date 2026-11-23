-- One session push on a person-sized fleet (ui/whole_list_fixture.luau:
-- the shell on the Control room, ~700 chats in the sidebar, every widget
-- shown): the page takes it without a script error, and the sidebar's rows
-- each follow their own session, a push changing one chat waking that
-- one's row at most. (Per-push run counts and times: measured on the same
-- fixture under zgx-run --inspect.)

local function open(): any
	local app = mount("whole_list_fixture.luau")
	app:resize(1440, 1000)
	app:frames(20)
	app:command("wl_big")
	app:frames(90)
	return app
end

local function rows_woken(app: any, command: string, n: number): number
	app:command("wl_reset")
	for _ = 1, n do
		app:command(command)
		app:frames(10)
	end
	app:command("wl_read")
	return ((app:state("wl_counts") or {})["sidebar.row"] or 0) / n
end

test("whole list: one chat's status, at most its own sidebar row woken", function()
	local app = open()
	expect(app:has('#sidebar >> "Own thread 1"')).to_be(true)
	expect(rows_woken(app, "wl_status", 4) <= 1).to_be(true)
	expect(app:error()).to_be_nil()
end)

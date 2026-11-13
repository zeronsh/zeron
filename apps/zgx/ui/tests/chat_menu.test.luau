-- A chat's menu (screens/shell/chat_menu): the sidebar's as it was, built
-- by the shared module the sidebar hands its dialogs to. On
-- ui/panel_home_fixture.luau, whose `session_menu` command lists the
-- sidebar's menu's labels for `test_menu_of` in `test_menu_labels`.

test("chat menu: the sidebar's session menu, from the shared builder", function()
	local app = mount("panel_home_fixture.luau")
	app:resize(1200, 800)
	app:frames(20)
	app:set_state("test_menu_of", "chat-1")
	app:command("session_menu")
	app:frames(2)
	local all = tostring(app:state("test_menu_labels"))
	for _, want in { "Rename", "Copy", "Archive", "Delete" } do
		expect(all).to_contain(want)
	end
	-- A side chat's: no Pin, no Move to (as before).
	app:set_state("test_menu_of", "side-a")
	app:command("session_menu")
	app:frames(2)
	local side = tostring(app:state("test_menu_labels"))
	expect(side).never.to_contain("Pin")
	expect(side).to_contain("Rename")
end)

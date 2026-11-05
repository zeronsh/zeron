-- "Open links in Zeron": a web link in a reply goes to the app's opener
-- (the shell's: the panel's browser tab) when one is set, and to the system
-- otherwise; a file link is never the opener's. On ui/chat_fixture.luau.

test("a web link in a reply goes to the app's opener once one is set", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:command("set_opener")
	app:command("link_turn")
	app:frames(30)
	app:click('role:link "the docs"')
	app:frames(5)
	expect(app:state("calls")).to_contain("opener:https://example.com/a.rs")
	expect(app:state("links_last")).to_be("app:https://example.com/a.rs")
	-- A prompt's web link too (the user's bubble, ds/user_message).
	app:click('role:link "the site"')
	app:frames(5)
	expect(app:state("calls")).to_contain("opener:https://example.com/")
	expect(app:state("links_last")).to_be("app:https://example.com/")
	-- A file link is the panel's, not the opener's.
	app:click('role:link "the parser"')
	app:frames(5)
	expect(app:state("calls")).to_contain("open_file:s2:src/parse.rs")
	expect(app:state("links_last")).to_be("app:https://example.com/")
end)

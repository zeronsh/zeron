-- A question of several pages steps back as Zeron's does (composer.rs
-- wizard `back`): Back, or Escape, from the second page asks the host to
-- take that page's answer off, and what was picked there shows chosen.

test("Back on a question's second page goes back with its pick", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:command("ask_question_page2")
	app:frames(30)
	expect(app:has('role:button "Back"')).to_be(true)
	app:click('role:button "Back"')
	app:frames(10)
	expect(app:state("calls")).to_contain("question_back:s2:q2")
	-- The first page again, its pick chosen; no Back on it.
	expect(app:tree("#chat-composer")).to_contain('radio "A new branch"')
	expect(app:tree("#chat-composer"):match('radio "A new branch"[^\n]*')).to_contain(" checked")
	expect(app:has('role:button "Back"')).to_be(false)
	expect(app:error()).to_be(nil)
end)

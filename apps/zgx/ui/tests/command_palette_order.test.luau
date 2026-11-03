-- Typing into the command palette over a root whose items are made fresh
-- each call (ui/command_palette_order_fixture.luau): narrowing a search
-- keeps hits of the previous list, which hold no place in the new one. The
-- build must not error, and the hits keep a sensible order (commands and
-- sessions both listed, none lost).

test("typing and narrowing over a fresh item list does not error", function()
	local app = mount("command_palette_order_fixture.luau")
	app:resize(900, 600)
	app:frames(10)
	expect(app:error()).to_be_nil()
	app:type("role:textbox", "r")
	app:frames(5)
	expect(app:error()).to_be_nil()
	app:type("role:textbox", "e")
	app:frames(5)
	expect(app:error()).to_be_nil()
	expect(app:has(`"Refresh models"`)).to_be(true)
	expect(app:has(`"Rebase the branch"`)).to_be(true)
	app:type("role:textbox", "t")
	app:frames(5)
	expect(app:error()).to_be_nil()
	expect(app:has(`"Retry uploads"`)).to_be(true)
end)

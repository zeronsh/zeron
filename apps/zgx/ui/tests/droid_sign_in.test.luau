-- Factory Droid signs in from its own CLI (`droid`, then /login): its card
-- says so where another agent's says "Connect an account", and offers no
-- sign-in of Zeron's. On the agents made up in ui/providers_fixture.luau.

test("Factory Droid's card says how it signs in, and offers no sign-in", function()
	local app = mount("providers_fixture.luau")
	app:resize(680, 1400)
	app:frames(30)
	expect(app:has('"Factory Droid"')).to_be(true)
	expect(app:has('"Signs in through Factory Droid"')).to_be(true)
	expect(app:has('"Run droid in a terminal, then /login. Zeron uses that sign-in."')).to_be(true)
	expect(app:has('"Connect a Factory Droid account"')).to_be(false)
	-- Another agent with no accounts still offers to connect one.
	expect(app:has('"Connect a Devin account"')).to_be(true)
	app:click('"Signs in through Factory Droid"')
	app:frames(20)
	expect(app:state("calls")).never.to_contain("sign_in:droid")
end)

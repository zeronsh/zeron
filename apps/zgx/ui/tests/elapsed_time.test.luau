-- A running timer counts to the second, however long it has run: seeing the
-- seconds go is how you know it's working.

test("a running timer always shows its seconds", function()
	local app = mount("elapsed_fixture.luau")
	app:frames(2)
	for _, said in { "42s", "1m 0s", "3m 22s", "1h 5m 22s" } do
		expect(app:has(`"{said}"`)).to_be(true)
	end
end)

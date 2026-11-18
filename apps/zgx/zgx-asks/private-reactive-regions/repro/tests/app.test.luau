-- Nothing inside a private element is a plugin's target, "however the inside
-- builds" (docs/design/plugin-hooks.md). On zgx b80a8af the part built bare
-- is kept from the plugin; those a presence, a show or an each builds are
-- not: the plugin replaces them and adds after them.
test("nothing in a private element is a plugin's target, however it's built", function()
	local app = mount("app.luau")
	app:frames(10)
	for _, id in { "bare", "in-presence", "in-show", "in-each-r1" } do
		expect({ id = id, replaced = app:has(`#replaced-{id}`), added = app:has(`#added-{id}`) }).to_equal({ id = id, replaced = false, added = false })
	end
end)

test("a node can't sit inside a paragraph's text", function()
	local app = mount("app.luau")
	app:resize(400, 300)
	app:frames(5)
	-- What zgx says of a node among a paragraph's spans.
	print("node in ui.rich: " .. tostring(app:state("node_in_rich")))
	expect(app:state("node_in_rich")).never.to_be("accepted")
	-- And the row: the last text doesn't wrap with the first.
	print(app:tree("#row"))
end)

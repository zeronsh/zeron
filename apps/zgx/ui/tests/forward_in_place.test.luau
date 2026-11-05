-- Setup's way on keeps its words as its choices change: another choice
-- arriving (an invitation accepted, its organization joining) leaves the
-- words already there the same nodes, and the pill as wide as its widest.
-- (The stack was built again whenever any choice changed.) On the pill in
-- ui/forward_fixture.luau.

-- A node's identity and box, by its kind and text: `@id` and `w`.
local function node(app: any, kind: string, label: string): (string?, number?)
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(` {kind} "{label}"`, 1, true) then
			return line:match("@(%d+)"), tonumber(line:match(" %-?%d+,%-?%d+ (%d+)x%d+"))
		end
	end
	return nil, nil
end

local function settle(app: any)
	app:frames(30)
end

test("a choice arriving leaves the others' words in place", function()
	local app = mount("forward_fixture.luau")
	app:resize(600, 300)
	settle(app)
	local words = node(app, "text", "Continue in Northwind Robotics")
	local _, pill = node(app, "button", "Continue in Halcyon Labs")
	expect(words).never.to_be_nil()
	expect(pill).to_be(290)

	app:set_state("test_forward_choices", { "Continue in Halcyon Labs", "Continue in Northwind Robotics", "Continue in Finch & Co" })
	settle(app)
	expect(`words @{node(app, "text", "Continue in Northwind Robotics")}`).to_be(`words @{words}`)
	local _, still = node(app, "button", "Continue in Halcyon Labs")
	expect(still).to_be(290)
	app:set_state("test_forward_pick", 3)
	settle(app)
	expect(app:has('role:button "Continue in Finch & Co"')).to_be(true)
end)

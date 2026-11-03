-- Gallery examples that change in place as they're used: choosing a colour
-- scheme keeps the step (and the picker clicked) as it is, its theme
-- following; an invitation answered keeps the organizations above it; a
-- search keeps the rows it still finds. (Each was built again for each
-- change: the scheme rebuilt the window, the picker too; the invitation's
-- answer the list; each letter typed the list.) On the examples in
-- ui/demo_fixture.luau, the one shown named by `test_demo`.

-- A node's identity, by its kind and label (or text): `@id`.
local function node(app: any, kind: string, label: string): string?
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(` {kind} "{label}"`, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

local function settle(app: any)
	app:frames(30)
end

local function same(app: any, kind: string, label: string, was: string?)
	expect(`{label} @{node(app, kind, label)}`).to_be(`{label} @{was}`)
end

test("a colour scheme chosen keeps the step and its picker", function()
	local app = mount("demo_fixture.luau")
	app:resize(1000, 760)
	settle(app)
	local group = node(app, "radiogroup", "Color scheme")
	local dark = node(app, "radio", "Dark")
	local accent = node(app, "radio", "Orange")
	expect(dark).never.to_be_nil()
	app:click('role:radio "Dark"')
	settle(app)
	same(app, "radiogroup", "Color scheme", group)
	same(app, "radio", "Dark", dark)
	same(app, "radio", "Orange", accent)
	expect(app:tree():find('radio "Dark" [^\n]* checked') ~= nil).to_be(true)
end)

test("an invitation answered keeps the organizations above it", function()
	local app = mount("demo_fixture.luau")
	app:resize(1000, 760)
	app:set_state("test_demo", "orgs")
	settle(app)
	local halcyon = node(app, "radio", "Halcyon Labs")
	local northwind = node(app, "radio", "Northwind Robotics")
	expect(halcyon).never.to_be_nil()
	app:click('role:button "Accept"')
	settle(app)
	same(app, "radio", "Halcyon Labs", halcyon)
	same(app, "radio", "Northwind Robotics", northwind)
	expect(node(app, "radio", "Finch & Co")).never.to_be_nil()
end)

test("a search keeps the rows it still finds", function()
	local app = mount("demo_fixture.luau")
	app:resize(1000, 760)
	app:set_state("test_demo", "members")
	settle(app)
	local theo = node(app, "text", "Theo Lindqvist")
	expect(theo).never.to_be_nil()
	app:type('role:textbox "Search"', "t")
	settle(app)
	same(app, "text", "Theo Lindqvist", theo)
	app:type('role:textbox "Search"', "h")
	settle(app)
	same(app, "text", "Theo Lindqvist", theo)
	expect(node(app, "text", "Priya Raman")).to_be_nil()

	app:set_state("test_demo", "plugins")
	settle(app)
	local linear = node(app, "text", "Linear")
	expect(linear).never.to_be_nil()
	app:type('role:textbox "Search"', "i")
	settle(app)
	same(app, "text", "Linear", linear)
end)

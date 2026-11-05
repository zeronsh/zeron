-- A session's card going as another comes (screens/shell/init): what leaves stays
-- beside its own row while it fades, wherever the new one stands.

local function place(app: any, title: string): (number?, number?)
	-- (The card's: the last of what's so named, the row's being first.)
	local x, y = nil, nil
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(`"{title}"`, 1, true) then
			local at_x, at_y = line:match(" (%-?%d+),(%-?%d+) ")
			x, y = tonumber(at_x), tonumber(at_y)
		end
	end
	return x, y
end

test("the card leaving stays where it was as the next comes", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(20)
	local first, second = { id = "a1", title = "Fix the build" }, { id = "a2", title = "Write the docs" }
	local function rest_on(c: any, y: number)
		app:set_state("screens/shell/sidebar.luau::shell_session_card", { id = c.id, x = 12, y = y, w = 280, h = 30 })
	end
	rest_on(first, 100)
	app:frames(30)
	local x1, y1 = place(app, first.title)
	expect(x1 ~= nil).to_be(true)
	rest_on(second, 400)
	app:frames(2)
	local x2, y2 = place(app, first.title)
	expect(x2).to_be(x1)
	expect(y2).to_be(y1)
end)

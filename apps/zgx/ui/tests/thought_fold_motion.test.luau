-- A thought's "Show all N lines" (ds/activity `thought_row`), toggled in its
-- tool group: the words' clip eases its height, and the group and what
-- follows it move with it, frame for frame. The button stays where it
-- stands at rest from the next text, never under it or cut by the group:
-- its box never meets the next row's. On the chat fixture's s6 (a thought,
-- then the reply's text).

-- A node's box, by a pattern on its tree line: y and height.
local function box(app: any, pattern: string): (number?, number?)
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			local y, h = line:match(" %-?%d+,(%-?%d+) %d+x(%d+)")
			return tonumber(y), tonumber(h)
		end
	end
	return nil, nil
end

-- From the button's foot to the next text's top.
local function gap(app: any): number
	local by, bh = box(app, 'button "Show')
	local ny = box(app, 'text "Layout is 61%')
	assert(by and bh and ny, "the button and the next text")
	return (ny :: number) - ((by :: number) + (bh :: number))
end

test("Show all and Show fewer move the button and what follows together, frame for frame", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s6")
	app:frames(30)
	app:click('role:button ~"Thought process"')
	app:frames(30)
	local rest = gap(app)
	expect(rest > 0).to_be(true)
	for _, pick in { 'role:button ~"Show all"', 'role:button "Show fewer lines"' } do
		app:click(pick)
		for frame = 1, 24 do
			app:frames(1)
			local now = gap(app)
			expect(`{pick}, frame {frame}: {now >= rest - 1 and now <= rest + 1}`).to_be(`{pick}, frame {frame}: true`)
		end
		app:frames(30)
		expect(math.abs(gap(app) - rest) <= 1).to_be(true)
	end
end)

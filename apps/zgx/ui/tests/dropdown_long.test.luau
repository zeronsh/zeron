-- A dropdown over thousands of items (a repository's branches) builds the
-- rows in view, not every one: opening it and typing in its search cost
-- what a screenful does. The keys still reach every row: round past the
-- first to the last, or down the whole list, each highlighted row scrolled
-- into view and picked by Enter. On ui/dropdown_long_fixture.luau.

local function settle(app: any)
	app:frames(30)
end

-- A node's box from its tree line (the first holding `text`, and `also`).
local function box(app: any, text: string, also: string?): { x: number, y: number, w: number, h: number }?
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(text, 1, true) and (also == nil or line:find(also, 1, true)) then
			local x, y, w, h = line:match("(%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
			if x then
				return { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number }
			end
		end
	end
	return nil
end

-- Whether the option named `name` (its tile's letter before it, or not)
-- shows whole inside the open list.
local function in_view(app: any, name: string): boolean
	local row = box(app, ' option "', `{name}"`) or box(app, ' option "', `{name} `)
	local list = box(app, " scroll ")
	if not row or not list then
		return false
	end
	return row.y >= list.y and row.y + row.h <= list.y + list.h
end

-- (The fixture's refs: the 2,000th is "origin/wing/dropdown-2000", the
-- 300th "origin/release/dropdown-300".)
test("long dropdown: a long list builds only the rows in view", function()
	local app = mount("dropdown_long_fixture.luau")
	app:resize(960, 900)
	app:set_state("test_long_n", 2000)
	settle(app)
	app:click('role:button "Branch"')
	settle(app)
	local built = app:count("role:option")
	expect(built > 0).to_be(true)
	expect(built < 40).to_be(true)
	expect(in_view(app, "gael/perf-1")).to_be(true)
	-- Typing narrows it, as few built.
	app:type('role:textbox "Search branches…"', "origin/")
	settle(app)
	built = app:count("role:option")
	expect(built > 0 and built < 40).to_be(true)
	expect(app:has('role:option "gael/perf-1 current"')).to_be(false)
	-- And cleared, the same window again.
	app:key("cmd+a")
	app:key("Backspace")
	settle(app)
	built = app:count("role:option")
	expect(built > 0 and built < 40).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("long dropdown: the keys round to the last of thousands, in view, and pick it", function()
	local app = mount("dropdown_long_fixture.luau")
	app:resize(960, 900)
	app:set_state("test_long_n", 2000)
	settle(app)
	app:click('role:button "Branch"')
	settle(app)
	-- On the chosen (the first), ↑ rounds to the last.
	app:key("ArrowUp")
	settle(app)
	expect(in_view(app, "origin/wing/dropdown-2000")).to_be(true)
	expect(app:has('role:option "gael/perf-1 current"')).to_be(false)
	app:key("Enter")
	settle(app)
	expect(app:state("test_long_branch")).to_be("origin/wing/dropdown-2000")

	-- Rows with a description (taller) and an action after the items: from
	-- the third, ↑ three times is the action, a fourth the last item.
	app:set_state("test_long_open", 1)
	settle(app)
	for _ = 1, 4 do
		app:key("ArrowUp")
	end
	settle(app)
	expect(in_view(app, "Item 2000")).to_be(true)
	app:key("Enter")
	settle(app)
	expect(app:state("test_long_value")).to_be("item-2000")
	expect(app:error()).to_be(nil)
end)

test("long dropdown: ↓ walks the whole list, each row brought into view", function()
	local app = mount("dropdown_long_fixture.luau")
	app:resize(960, 900)
	app:set_state("test_long_n", 300)
	settle(app)
	app:click('role:button "Branch"')
	settle(app)
	for i = 1, 299 do
		app:key("ArrowDown")
		if i % 50 == 0 then
			app:frames(2)
		end
	end
	settle(app)
	expect(in_view(app, "origin/release/dropdown-300")).to_be(true)
	app:key("Enter")
	settle(app)
	expect(app:state("test_long_branch")).to_be("origin/release/dropdown-300")
	expect(app:error()).to_be(nil)
end)

-- A menu's list follows its lit row (ds/menu `reveal`): ↓ and ↑ past the
-- rows that show scroll the list with it, round its ends too, and a new
-- query or scope starts it at its top. The composer's `/` list
-- (ds/composer_mentions; `$` is the same list) and its `@` launcher
-- (ds/mention_launcher, a heading over each section), on the chat page
-- (ui/chat_fixture.luau).

local BOX = 'role:textbox "Message"'

local function settle(app: any)
	app:frames(30)
end

local function chat(): any
	local app = mount("chat_fixture.luau")
	app:resize(1000, 900)
	app:command("go_s2")
	settle(app)
	return app
end

local function box(line: string): (number, number, number, number)
	local x, y, w, h = line:match(" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
	return tonumber(x) :: number, tonumber(y) :: number, tonumber(w) :: number, tonumber(h) :: number
end

-- Whether row `i` (`#mention-<i>`) stands whole in the list's view (its
-- scroll, the listbox), as the tree lays them out.
local function shows(app: any, i: number, case: string)
	local tree = app:tree("#mention-popover")
	local top, bottom, y, b = nil, nil, nil, nil
	for line in tree:gmatch("[^\n]+") do
		if line:find(" listbox ", 1, true) then
			local _, ly, _, lh = box(line)
			top, bottom = ly, ly + lh
		elseif line:find(`#mention%-{i}%f[^%w%-]`) then
			local _, ly, _, lh = box(line)
			y, b = ly, ly + lh
		end
	end
	if not top or not bottom or not y or not b then
		error(`{case}: no list or no row {i} in:\n{tree}`)
	end
	local inside = (y :: number) >= (top :: number) - 0.5 and (b :: number) <= (bottom :: number) + 0.5
	expect(`{case}: row {i} at {y}..{b} in {top}..{bottom}: {inside}`).to_be(`{case}: row {i} at {y}..{b} in {top}..{bottom}: true`)
end

local function key(app: any, k: string)
	app:key(k)
	settle(app)
end

-- Down to the last row and round to the first, up round to the last and
-- back to the first: the lit row in view at each step.
local function walk(app: any, what: string, rows: number): number
	local n = app:count("#mention-popover >> role:option")
	-- (More rows than show: the list scrolls.)
	expect(`{what}: {n} rows, more than {rows}: {n > rows}`).to_be(`{what}: {n} rows, more than {rows}: true`)
	shows(app, 1, `{what} opened`)
	for i = 2, n do
		key(app, "ArrowDown")
		shows(app, i, `{what} down`)
	end
	key(app, "ArrowDown")
	shows(app, 1, `{what} round to the first`)
	key(app, "ArrowUp")
	shows(app, n, `{what} round to the last`)
	for i = n - 1, 1, -1 do
		key(app, "ArrowUp")
		shows(app, i, `{what} up`)
	end
	return n
end

test("the / list scrolls with its lit row, down past what shows and round its ends; a new query starts at its top", function()
	local app = chat()
	app:type(BOX, "/")
	settle(app)
	local n = walk(app, "/", 7)
	-- Far down, then a letter typed and taken back: the first row lit, in
	-- view.
	for _ = 2, n do
		key(app, "ArrowDown")
	end
	shows(app, n, "/ at its end")
	app:type(BOX, "c")
	settle(app)
	key(app, "Backspace")
	shows(app, 1, "/ typed at")
end)

test("the @ launcher scrolls with its lit row through its sections; a scope switched to starts at its top", function()
	local app = chat()
	app:command("drop_pictures")
	settle(app)
	app:type(BOX, "@")
	settle(app)
	local n = walk(app, "@ all", 8)
	for _ = 2, n do
		key(app, "ArrowDown")
	end
	shows(app, n, "@ at its end")
	key(app, "Tab")
	shows(app, 1, "@ a scope")
end)

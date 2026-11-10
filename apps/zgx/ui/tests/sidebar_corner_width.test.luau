-- A row's corner (its time at rest, its state, the archive button under the
-- pointer, its jump keys under ⌘) has one fixed width, its content to the
-- right: what sits before it, the subagents pill, keeps its column whatever
-- the time ("5m", "12h", "104w") and with the pointer on the row. (⌘ held,
-- the jump keys, wider, widen the corner while shown.) On the shell in ui/shell_fixture.luau (`corner_width`).

local SIDE = "#shell-sidebar"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("corner_width")
	app:frames(40)
	return app
end

-- How far in a row's subagents pill stands (its x), by the row's title.
local function pill(app: any, title: string): number
	for line in app:tree(`{SIDE} >> role:button "{title}"`):gmatch("[^\n]+") do
		if line:find("subagents running", 1, true) then
			return tonumber(line:match(" (%-?%d+),%-?%d+ %d+x%d+")) :: number
		end
	end
	error(`no subagents pill on "{title}"`)
end

test("the subagents pill keeps its column whatever the time in the corner: 5m, 12h, 104w", function()
	local app = open()
	local x = pill(app, "Fix the build")
	expect(pill(app, "Write the docs")).to_be(x)
	expect(pill(app, "Tidy the tests")).to_be(x)
end)

test("the pill stays put as the pointer arrives", function()
	local app = open()
	local x = pill(app, "Write the docs")
	app:hover(`{SIDE} >> role:button "Write the docs"`)
	app:frames(20)
	expect(pill(app, "Write the docs")).to_be(x)
	expect(pill(app, "Tidy the tests")).to_be(x)
end)

-- The corner takes the room of its widest resting time ("104w") and no
-- more: the gap from the pill to the time is 4 points.
local function gap_to_time(app: any, title: string, time: string): number
	local right = 0
	local left = 0
	for line in app:tree(`{SIDE} >> role:button "{title}"`):gmatch("[^\n]+") do
		local x, w = line:match(" (%-?%d+),%-?%d+ (%d+)x%d+")
		if line:find("subagents running", 1, true) then
			right = tonumber(x) + tonumber(w)
		elseif line:find(` text "{time}"`, 1, true) then
			left = tonumber(x) :: number
		end
	end
	return left - right
end

test("the corner takes the room of its widest resting time, no more", function()
	local app = open()
	expect(gap_to_time(app, "Tidy the tests", "104w") <= 4).to_be(true)
end)

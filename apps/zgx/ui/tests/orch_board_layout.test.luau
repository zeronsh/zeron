-- The Board's columns use the width they're given (ds/orch_board): side by
-- side where all fit, else balanced rows of equal columns (an empty Blocked
-- holding no place), never narrower than a card fits, every row as wide as
-- the board. On the Control room's Board tab and on an orchestrator's page,
-- at 900, 1200 and 1440 points.

local function boxes(app: any, target: string): { { x: number, y: number, w: number, h: number } }
	local out = {}
	for line in app:tree(target, 1):gmatch("[^\n]+") do
		if not string.find(line, " hidden", 1, true) then
			local x, y, w, h = string.match(line, " (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
			if x then
				table.insert(out, { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number })
			end
		end
	end
	return out
end

-- The board's box, and its standing columns' boxes.
local function layout(app: any, board: string): (any, { any })
	local b = boxes(app, board)[1]
	local cols = {}
	for i = 1, app:count(`{board} >> .orch-column`) do
		local c = boxes(app, `{board} >> .orch-column [{i}]`)[1]
		if c and c.w > 0 and c.h > 0 then
			table.insert(cols, c)
		end
	end
	return b, cols
end

local function check(app: any, board: string, label: string)
	local b, cols = layout(app, board)
	expect(#cols >= 3).to_be(true)
	local w0 = cols[1].w
	-- One under another (no room for two): each the board's width.
	if math.abs(w0 - b.w) < 2 then
		for _, c in cols do
			expect(math.abs(c.w - b.w) < 2).to_be(true)
		end
		return
	end
	expect(w0 >= 219).to_be(true)
	local rows: { [number]: { any } } = {}
	for _, c in cols do
		-- Every one as wide as the first.
		if math.abs(c.w - w0) >= 1 then
			print(`{label}: column {c.x},{c.y} {c.w} vs {w0}`)
		end
		expect(math.abs(c.w - w0) < 1).to_be(true)
		local key = math.floor(c.y + 0.5)
		rows[key] = rows[key] or {}
		table.insert(rows[key], c)
	end
	-- Every row but the last full: the first's columns reach the board's
	-- right edge; none leaves room past it for another.
	local ys = {}
	for y in rows do
		table.insert(ys, y)
	end
	table.sort(ys)
	local first = rows[ys[1]]
	local right = 0
	for _, c in first do
		right = math.max(right, c.x + c.w)
	end
	expect(b.x + b.w - right < 2).to_be(true)
end

for _, w in { 900, 1200, 1440 } do
	test(`board layout: the Control room's Board at {w}`, function()
		local app = mount("control_page_fixture.luau")
		app:set_state("control_tab", "board")
		app:resize(w, 1400)
		app:frames(150)
		check(app, "#control-board", `control {w}`)
		expect(app:error()).to_be_nil()
	end)
	test(`board layout: an orchestrator's page's Board at {w}`, function()
		local app = mount("orchestrator_fixture.luau")
		app:resize(w, 1400)
		-- (The side panel alone, at every width: the Board in its Tasks.)
		app:set_state("orch_page_mode", "live")
		app:frames(150)
		check(app, "#orch-page-board", `orch {w}`)
	end)
end

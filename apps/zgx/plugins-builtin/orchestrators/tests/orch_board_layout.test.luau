-- The Board's columns use the width they're given (ds/orch_board): three
-- side by side, each as wide as the next, where three fit at 240 points
-- each; else one under another, each as wide as the board's lanes. On the
-- Control room's Board tab at 900, 1200 and 1440 points. (An orchestrator's
-- page shows its cards as the list, ds/orch_board_list: no columns.)

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

-- The lanes' box, and the columns' boxes.
local function layout(app: any, board: string): (any, { any })
	local lanes = boxes(app, `{board} >> #orch-columns`)[1]
	local cols = {}
	for _, c in { "needs_you", "running", "done" } do
		table.insert(cols, boxes(app, `{board} >> #orch-col-{c}`)[1])
	end
	return lanes, cols
end

local function check(app: any, board: string, label: string)
	local lanes, cols = layout(app, board)
	expect(#cols).to_be(3)
	local w0 = cols[1].w
	if lanes.w < 240 * 3 + 16 then
		-- One under another: each the lanes' width.
		for i, c in cols do
			expect(math.abs(c.w - lanes.w) < 2).to_be(true)
			expect(math.abs(c.x - lanes.x) < 1).to_be(true)
			if i > 1 then
				expect(c.y > cols[i - 1].y).to_be(true)
			end
		end
		return
	end
	expect(w0 >= 240).to_be(true)
	for i, c in cols do
		expect(if math.abs(c.w - w0) < 1 then "" else `{label}: column {c.x},{c.y} {c.w} vs {w0}`).to_be("")
		expect(math.abs(c.y - cols[1].y) < 1).to_be(true)
		if i > 1 then
			expect(c.x > cols[i - 1].x).to_be(true)
		end
	end
	-- From the lanes' left edge to their right.
	expect(math.abs(cols[1].x - lanes.x) < 2).to_be(true)
	expect(lanes.x + lanes.w - (cols[3].x + cols[3].w) < 2).to_be(true)
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
end

test("board layout: the Control room's Board narrow, its columns one under another", function()
	local app = mount("control_page_fixture.luau")
	app:set_state("control_tab", "board")
	app:resize(700, 2400)
	app:frames(150)
	local lanes = layout(app, "#control-board")
	expect(lanes.w < 240 * 3 + 16).to_be(true)
	check(app, "#control-board", "control 700")
	expect(app:error()).to_be_nil()
end)

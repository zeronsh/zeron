-- Actual layout: shared column widths, growing rows, and overflow only at
-- readable minimums, in both the preview and windowed/streaming Markdown.
local function box(app: any, target: string): { x: number, y: number, w: number, h: number }
	local line = app:tree(target):match("[^\n]+") or ""
	local x, y, w, h = line:match(" ([%-%d.]+),([%-%d.]+) ([%d.]+)x([%d.]+)")
	assert(x and y and w and h, line)
	return { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number }
end

local function mount_table(windowed: boolean): any
	local app = mount("markdown_table_fixture.luau")
	app:set_state("table_test_windowed", windowed)
	app:resize(600, 1200)
	app:frames(20)
	expect(app:error()).to_be_nil()
	return app
end

for _, windowed in { false, true } do
	local mode = if windowed then "reply" else "preview"
	test(`markdown table {mode}: content proportions, wrapping and aligned rows survive resize`, function()
		local app = mount_table(windowed)
		local wide = box(app, ".md-table-cell[4]")
		local short = box(app, ".md-table-cell[1]")
		local long = box(app, ".md-table-cell[2]")
		local frame = box(app, ".md-table")
		local first_width = long.x - short.x
		expect(frame.w - first_width > first_width).to_be(true)
		app:resize(300, 1200)
		app:frames(20)
		local a = box(app, ".md-table-cell[3]")
		local b = box(app, ".md-table-cell[4]")
		local next_row = box(app, ".md-table-row[3]")
		expect(b.h > wide.h).to_be(true)
		expect(a.y).to_be(b.y)
		expect(b.h > a.h).to_be(true)
		expect(next_row.y >= b.y + b.h + 24).to_be(true)
		expect(a.x).to_be(box(app, ".md-table-cell[1]").x)
		expect(b.x).to_be(box(app, ".md-table-cell[2]").x)
		expect(b.x + b.w + 12 <= 300).to_be(true)
		app:resize(600, 1200)
		app:frames(20)
		expect(box(app, ".md-table-cell[4]").h).to_be(wide.h)
		expect(app:error()).to_be_nil()
	end)

	test(`markdown table {mode}: narrow panels scroll at column minimums`, function()
		local app = mount_table(windowed)
		app:set_state("table_test_text", "| ID | Description | State |\n| --- | --- | --- |\n| 1 | A description long enough to need wrapping. | Ready |")
		app:resize(150, 1200)
		app:frames(20)
		local frame = box(app, ".md-table")
		local a = box(app, ".md-table-cell[1]")
		local b = box(app, ".md-table-cell[2]")
		expect(frame.w <= 150).to_be(true)
		expect(b.x - a.x >= 72).to_be(true)
		local c = box(app, ".md-table-cell[3]")
		expect(c.x - b.x >= 96).to_be(true)
		expect(c.x > frame.x + frame.w).to_be(true)
		expect(app:error()).to_be_nil()
	end)
end

test("markdown table reply: streaming content remeasures columns and row heights", function()
	local app = mount_table(true)
	app:set_state("table_test_text", "| ID | Description |\n| --- | --- |\n| 1 | Short |")
	app:resize(300, 1200)
	app:frames(20)
	local short = box(app, ".md-table-cell[4]").h
	app:set_state("table_test_text", "| ID | Description |\n| --- | --- |\n| 1 | Short, then more words arriving in the same cell until this description wraps over several visual lines. |\n| 2 | Next row |")
	app:frames(20)
	expect(box(app, ".md-table-cell[4]").h > short).to_be(true)
	expect(app:count(".md-table-row")).to_be(3)
	expect(app:error()).to_be_nil()
end)

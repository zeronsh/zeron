-- What runs wider than a reply's column (ds/wide): tables and code take
-- the page's spare width, centred on the column, then scroll sideways; a
-- table's cells wrap past their column's measure; code keeps to the
-- column and scrolls. On the transcript in ui/wide_fixture.luau: a narrow
-- table, a wide one, a very wide one, one in a list; long code and short.

local function box(line: string): { number }
	local x, y, w, h = line:match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
	return { tonumber(x) :: number, tonumber(y) :: number, tonumber(w) :: number, tonumber(h) :: number }
end

-- A table's or code block's view, inside its 1-point edge: the sideways
-- scroll in it.
local function of(app: any, target: string): { number }
	return box(app:tree(target):match("[^\n]* scroll [^\n]*"))
end

-- A text's box under a target.
local function text_in(app: any, target: string, words: string): { number }
	local line = app:tree(target):match('[^\n]*text "' .. words .. '[^\n]*')
	assert(line, `no text "{words}" in {target}`)
	return box(line)
end

local function open(width: number?): any
	local app = mount("wide_fixture.luau")
	app:resize(1400, 1000)
	if width then
		app:set_state("fixture_width", width)
	end
	app:frames(30)
	return app
end

-- The reading column, inside a frame's edge: the person's message keeps
-- to the column.
local function column(app: any): { number }
	local b = box(app:tree("#u1"):match("^[^\n]+"))
	return { b[1] + 1, b[2], b[3] - 2, b[4] }
end

test("a narrow table and short code keep to the reading column", function()
	local app = open()
	local col = column(app)
	expect(col[3]).to_be(660 - 2)
	local narrow = of(app, ".md-table[1]")
	expect(`{narrow[1]} {narrow[3]}`).to_be(`{col[1]} {col[3]}`)
	local short = of(app, ".md-code[2]")
	expect(`{short[1]} {short[3]}`).to_be(`{col[1]} {col[3]}`)
end)

test("a wide table takes the page's room, centred on the column, only as wide as it needs", function()
	local app = open()
	local col = column(app)
	local wide = of(app, ".md-table[2]")
	expect(wide[3] > col[3]).to_be(true)
	-- The page: the transcript's 1280 less its rows' sides.
	expect(wide[3] < 1280 - 50).to_be(true)
	expect(math.abs((wide[1] + wide[3] / 2) - (col[1] + col[3] / 2)) <= 1).to_be(true)
end)

test("a table's long cells wrap at its column measure; short ones keep to a line", function()
	local app = open()
	local long = text_in(app, ".md-table[2]", "Deployed to every region")
	expect(long[3] <= 320).to_be(true)
	expect(long[4] > 20).to_be(true)
	local short = text_in(app, ".md-table[2]", "Healthy")
	expect(short[4]).to_be(20)
	-- Each row as tall as its tallest cell: the rows' cells start together.
	expect(text_in(app, ".md-table[2]", "edge")[2]).to_be(long[2])
end)

test("a table wider than the page runs to the page's edges and scrolls sideways under the wheel", function()
	local app = open()
	local col = column(app)
	local very = of(app, ".md-table[3]")
	expect(very[3]).to_be(1280 - 50)
	expect(math.abs((very[1] + very[3] / 2) - (col[1] + col[3] / 2)) <= 1).to_be(true)
	local before = text_in(app, ".md-table[3]", "Region")
	local page = text_in(app, "#transcript", "Every region")
	app:scroll(".md-table[3]", 100)
	app:frames(10)
	local after = text_in(app, ".md-table[3]", "Region")
	expect(before[1] - after[1]).to_be(100)
	-- The page stayed where it was.
	expect(text_in(app, "#transcript", "Every region")[2]).to_be(page[2])
	-- Its rail, under the pointer.
	expect(app:count(".wide-rail") >= 3).to_be(true)
end)

test("the wheel over a table that fits scrolls the page", function()
	local app = open()
	local page = text_in(app, "#transcript", "Every region")
	app:scroll(".md-table[2]", -100)
	app:frames(10)
	expect(text_in(app, "#transcript", "Every region")[2] > page[2]).to_be(true)
end)

test("inside a list, a table keeps to the column", function()
	local app = open()
	local col = column(app)
	local listed = of(app, ".md-table[4]")
	expect(listed[1] >= col[1]).to_be(true)
	expect(listed[1] + listed[3] <= col[1] + col[3]).to_be(true)
end)

test("long code keeps to the column and scrolls sideways under the wheel; fitted, it wraps", function()
	local app = open()
	local col = column(app)
	local long = of(app, ".md-code[1]")
	expect(`{long[1]} {long[3]}`).to_be(`{col[1]} {col[3]}`)
	local before = text_in(app, ".md-code[1]", "fn main")
	app:scroll(".md-code[1]", 100)
	app:frames(10)
	expect(before[1] - text_in(app, ".md-code[1]", "fn main")[1]).to_be(100)
	local one = text_in(app, ".md-code[1]", "fn main")
	app:click('role:checkbox "Fit content"[1]')
	app:frames(10)
	-- Fitted: no sideways view, its lines wrapping inside the column.
	expect(app:tree(".md-code[1]"):find(" scroll ", 1, true)).to_be_nil()
	local wrapped = text_in(app, ".md-code[1]", "fn main")
	expect(wrapped[4] > one[4]).to_be(true)
	expect(wrapped[1] + wrapped[3] <= col[1] + col[3]).to_be(true)
	app:click('role:checkbox "Fit content"[1]')
	app:frames(10)
end)

test("a page no wider than the column leaves no room: everything keeps to it", function()
	local app = open(700)
	local col = column(app)
	expect(col[3]).to_be(700 - 50)
	for _, t in { ".md-table[2]", ".md-table[3]", ".md-code[1]" } do
		local b = of(app, t)
		expect(`{t} {b[1]} {b[3]}`).to_be(`{t} {col[1]} {col[3]}`)
	end
	-- The very wide table scrolls within it.
	local before = text_in(app, ".md-table[3]", "Region")
	app:scroll(".md-table[3]", 100)
	app:frames(10)
	expect(before[1] - text_in(app, ".md-table[3]", "Region")[1]).to_be(100)
end)

test("the page widening gives a wide table more room, at once", function()
	local app = open(1000)
	local narrower = of(app, ".md-table[3]")
	expect(narrower[3]).to_be(1000 - 50)
	app:set_state("fixture_width", 1280)
	app:frames(10)
	expect(of(app, ".md-table[3]")[3]).to_be(1280 - 50)
end)

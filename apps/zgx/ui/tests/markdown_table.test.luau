-- A reply's tables (ds/markdown): they fit the reply's column whenever they
-- can, wrapping prose and keeping short columns whole, and scroll sideways
-- only when they can't, by a trackpad's swipe, Shift and the wheel, or the
-- wheel itself once the pointer has been moved onto the table. The page
-- around them keeps its own scrolling. (table_fixture.luau.)

-- A node's box, from the tree's first line: `@n … x,y WxH`.
local function box(app: any, target: string): { x: number, y: number, w: number, h: number }
	local x, y, w, h = app:tree(target):match("^%s*@%d+[^\n]- ([%-%d.]+),([%-%d.]+) ([%d.]+)x([%d.]+)")
	return { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number }
end

local function only(name: string, width: number?): any
	local app = mount("table_fixture.luau")
	app:resize(960, 640)
	app:command(`only_{name}`)
	app:command(`width_{width or 760}`)
	app:frames(10)
	expect(app:error()).to_be_nil()
	return app
end

-- A cell's words, in reading order with the header first: `count` columns
-- a row.
local function words(app: any, count: number, r: number, c: number): { x: number, y: number, w: number, h: number }
	return box(app, `.md-text >> [{r * count + c}]`)
end
-- How far a table's content has moved sideways (its first words start a
-- border and a cell's padding in), and the page down.
local function across(app: any): number
	return box(app, ".md-table").x + 17 - words(app, 1, 0, 1).x
end
local function down(app: any): number
	return -box(app, "#reply").y
end
-- How far it can move: where its last header ends, past where the view does.
-- (The tree's sizes are rounded: within a point counts as there.)
local function room(app: any, count: number): number
	local view, last = box(app, ".md-table"), words(app, 1, 0, count)
	return last.x + last.w + 16 + across(app) - (view.x + view.w - 1)
end
local function near(a: number, b: number)
	expect(math.abs(a - b) <= 1.5).to_be(true)
end

-- Past the time a gesture holds on to what it scrolled.
local function pause(app: any)
	app:wait("#never-shown", { timeout_ms = 360 })
	app:frames(2)
end

local function fits(app: any, count: number)
	expect(room(app, count) <= 1).to_be(true)
end

test("table: two columns of prose fit the reply, wrapped, rather than running out of it", function()
	local app = only("two")
	fits(app, 2)
	-- Wrapped: a row taller than the header's one line…
	expect(words(app, 2, 1, 1).h > words(app, 2, 0, 1).h).to_be(true)
	-- …and each column has a real share of the width.
	local whole = box(app, ".md-table").w
	expect(words(app, 2, 1, 1).w > whole * 0.3).to_be(true)
	expect(words(app, 2, 1, 2).w > whole * 0.3).to_be(true)
end)

test("table: a 30-character label beside prose fits the reply at its widths, wrapped", function()
	for _, width in { 480, 620, 760 } do
		local app = only("running", width)
		fits(app, 2)
		-- The prose wrapped inside the view, not past it.
		local view, prose = box(app, ".md-table"), words(app, 2, 1, 2)
		expect(prose.x + prose.w <= view.x + view.w).to_be(true)
		expect(prose.h > words(app, 2, 0, 2).h).to_be(true)
	end
end)

test("table: the same table scrolls, rather than clips, once its columns' least widths can't fit", function()
	local app = only("running", 280)
	-- (Its prose runs past the view: the header's words end inside it.)
	local view, prose = box(app, ".md-table"), words(app, 2, 1, 2)
	expect(prose.x + prose.w + 16 > view.x + view.w).to_be(true)
	app:wheel(".md-table", { dy = 40, mouse = true, shift = true })
	expect(across(app)).to_be(40)
	expect(down(app)).to_be(0)
end)

test("table: short columns keep to one line and the prose beside them takes the rest", function()
	local app = only("mixed")
	fits(app, 4)
	-- The notes wrapped; the labels and numbers beside them didn't: their
	-- words are as tall as the header's one line, in a row the notes made
	-- taller.
	local line = words(app, 4, 0, 1).h
	expect(words(app, 4, 1, 4).h > line).to_be(true)
	expect(words(app, 4, 1, 1).h).to_be(line)
	expect(words(app, 4, 1, 2).h).to_be(line)
	expect(words(app, 4, 1, 3).h).to_be(line)
end)

test("table: a table narrower than the reply still runs its rules to its frame", function()
	local app = only("loose")
	-- Its prose column takes what the words leave, so the numbers, on their
	-- cells' right edges, end as far in from the frame's right as the first
	-- words start from its left: the cells, and the rules under them, reach
	-- the frame.
	local frame, first, last = box(app, ".md-table"), words(app, 2, 0, 1), words(app, 2, 3, 2)
	near(frame.x + frame.w - (last.x + last.w), first.x - frame.x)
end)

test("table: a column of numbers sits on its right edge, whether the source says so or not", function()
	local app = only("mixed")
	-- `Size` says `---:`, `Change` too; each number's right edge is its
	-- header's, whatever their widths.
	local head, number = words(app, 4, 0, 2), words(app, 4, 1, 2)
	near(head.x + head.w, number.x + number.w)
	expect(head.w ~= number.w).to_be(true)
	-- `Build` has no numbers, and starts on its left.
	near(words(app, 4, 0, 1).x, words(app, 4, 1, 1).x)
end)

test("table: a column of numbers the source doesn't mark sits on its right too", function()
	local app = only("many")
	local head, number = words(app, 8, 0, 4), words(app, 8, 1, 4)
	near(head.x + head.w, number.x + number.w)
	near(words(app, 8, 0, 1).x, words(app, 8, 1, 1).x)
end)

test("table: too many columns to fit keep their words whole and scroll", function()
	local app = only("many")
	expect(room(app, 8) > 0).to_be(true)
	-- Not squeezed to a word a line first: its short cells keep to a line.
	expect(words(app, 8, 1, 1).h).to_be(words(app, 8, 0, 1).h)
end)

test("table: code that can't break scrolls, and the table says so", function()
	local app = only("code")
	local view, code = box(app, ".md-table"), words(app, 2, 1, 2)
	expect(code.x + code.w > view.x + view.w).to_be(true)
end)

test("table: a narrow window wraps what it can before scrolling", function()
	local app = only("two", 360)
	fits(app, 2)
end)

test("table: a table that fits never takes the wheel", function()
	local app = only("two")
	app:hover(".md-table")
	app:wheel(".md-table", { dy = 80, mouse = true })
	expect(across(app)).to_be(0)
	expect(down(app) > 0).to_be(true)
end)

test("table: a trackpad's swipe moves the table, its drift downward included, and not the page", function()
	local app = only("many")
	for _ = 1, 6 do
		app:wheel(".md-table", { dx = 12, dy = 2 })
	end
	expect(across(app)).to_be(72)
	expect(down(app)).to_be(0)
end)

test("table: a trackpad's swipe down moves the page, and the table not at all", function()
	local app = only("many")
	for _ = 1, 4 do
		app:wheel(".md-table", { dx = 2, dy = 12 })
	end
	expect(across(app)).to_be(0)
	expect(down(app)).to_be(48)
end)

test("table: a sideways moment in a swipe down leaves the table alone", function()
	local app = only("many")
	app:wheel(".md-table", { dy = 12 })
	app:wheel(".md-table", { dx = 3, dy = 1 })
	app:wheel(".md-table", { dy = 12 })
	expect(across(app)).to_be(0)
	expect(down(app)).to_be(25)
end)

test("table: a trackpad's straight swipe down over a wide table is the page's, pointer moved or not", function()
	local app = only("many")
	app:hover(".md-table")
	app:wheel(".md-table", { dy = 40 })
	expect(across(app)).to_be(0)
	expect(down(app)).to_be(40)
end)

test("table: the wheel moves a table the pointer was moved onto, then the page at its end", function()
	local app = only("many")
	local room0 = room(app, 8)
	app:hover(".md-table")
	app:wheel(".md-table", { dy = 40, mouse = true })
	expect(across(app)).to_be(40)
	expect(down(app)).to_be(0)
	-- On to its end, where it stops: a fling doesn't run on into the page…
	for _ = 1, 40 do
		app:wheel(".md-table", { dy = 40, mouse = true })
	end
	near(across(app), room0)
	expect(down(app)).to_be(0)
	-- …and the wheel, turned again after a moment, moves the page.
	pause(app)
	app:wheel(".md-table", { dy = 40, mouse = true })
	expect(down(app)).to_be(40)
end)

test("table: the wheel going back up at a table's start is the page's at once", function()
	local app = only("many")
	app:hover("#below")
	app:wheel("#below", { dy = 100 })
	pause(app)
	app:hover(".md-table")
	app:wheel(".md-table", { dy = -40, mouse = true })
	expect(across(app)).to_be(0)
	expect(down(app)).to_be(60)
end)

test("table: a table that scrolled under a resting pointer leaves the wheel to the page", function()
	local app = only("many")
	-- The pointer on the page just below the table, then the table brought
	-- under it by scrolling; a pause, as one reading would.
	app:hover("#below")
	app:wheel("#below", { dy = 80 })
	pause(app)
	local before = down(app)
	app:wheel(".md-table", { dy = 40, mouse = true })
	expect(across(app)).to_be(0)
	expect(down(app)).to_be(before + 40)
end)

test("table: a gesture on the page stays the page's as a table slides under it", function()
	local app = only("many")
	app:hover("#below")
	app:wheel("#below", { dy = 40, mouse = true })
	app:wheel(".md-table", { dy = 40, mouse = true })
	expect(across(app)).to_be(0)
	expect(down(app)).to_be(80)
end)

test("table: Shift and the wheel move the table sideways, and never the page", function()
	local app = only("many")
	local room0 = room(app, 8)
	-- With no pointer move: Shift says what's meant by itself.
	app:wheel(".md-table", { dy = 40, mouse = true, shift = true })
	expect(across(app)).to_be(40)
	expect(down(app)).to_be(0)
	for _ = 1, 40 do
		app:wheel(".md-table", { dy = 40, mouse = true, shift = true })
	end
	near(across(app), room0)
	expect(down(app)).to_be(0)
end)

test("table: Shift's wheel on a Mac, which arrives as motion sideways, moves it too", function()
	local app = only("many")
	app:wheel(".md-table", { dx = 40, mouse = true, shift = true })
	expect(across(app)).to_be(40)
	expect(down(app)).to_be(0)
end)

test("table: a data table's long header wraps, whole, rather than being cut", function()
	local app = only("data", 360)
	local narrow = box(app, ".data-table-head")
	expect(narrow.h > 44).to_be(true)
	expect(app:has('~"Allocations per streamed token"')).to_be(true)
end)

test("table: words the source sets on their right sit there", function()
	local app = only("said")
	local head, free, team = words(app, 2, 0, 1), words(app, 2, 1, 1), words(app, 2, 2, 1)
	near(head.x + head.w, free.x + free.w)
	near(head.x + head.w, team.x + team.w)
	expect(free.x > team.x).to_be(true)
end)

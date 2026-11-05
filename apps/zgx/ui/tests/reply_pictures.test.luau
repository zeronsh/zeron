-- A reply's pictures as designed (ds/reply_picture, ds/markdown_find's parse;
-- ui/reply_pictures_fixture.luau, a 660-point reading column): one after a
-- line of words is drawn, not its alt text; several together share a tray in
-- justified rows; a long strip, a long page and an icon each take their own
-- frame; one still coming holds a 16:10 frame; one that can't be read is a
-- quiet line, its reason never drawn in the reply; a refused one stays the
-- link written; pictures an agent shows sit in the reply; a failed display
-- call is a line in plain words; and a picture already shown isn't built
-- again while the reply is written.

local function open(): any
	local app = mount("reply_pictures_fixture.luau")
	-- (Tall enough that the list builds every reply.)
	app:resize(1000, 7000)
	app:frames(30)
	expect(app:error()).to_be_nil()
	return app
end

-- Every node's box whose line holds `label` (in tree order): x, y, w, h,
-- and its id.
local function boxes(app: any, label: string): { { x: number, y: number, w: number, h: number, id: string } }
	local out = {}
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(label, 1, true) then
			local id, x, y, w, h = line:match("(@%d+).- (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
			if id then
				table.insert(out, { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number, id = id })
			end
		end
	end
	return out
end

local function box(app: any, label: string): { x: number, y: number, w: number, h: number, id: string }
	local all = boxes(app, label)
	expect(#all > 0).to_be(true)
	return all[#all]
end

local function near(a: number, b: number): boolean
	return math.abs(a - b) <= 1.5
end

test("a picture on the line under some words is drawn, not its alt text", function()
	local app = open()
	-- (The reply's first picture, and the run's first: both The shell.)
	expect(#boxes(app, 'button "Open The shell"')).to_be(2)
	expect(app:has('"Here it is after the change:"')).to_be(true)
	expect(app:has('"The sidebar keeps its width now."')).to_be(true)
end)

test("a picture among words is drawn between them; two with a space between share a tray; one it won't read stays its link", function()
	local app = open()
	local before = box(app, '"The menu bar "')
	local menu = box(app, 'button "Open The menu"')
	local mid = box(app, '" sits above it, and these "')
	local left = box(app, 'button "Open Left"')
	local right = box(app, 'button "Open Right"')
	local after = box(app, '" close the line."')
	expect(before.y < menu.y and menu.y + menu.h < mid.y).to_be(true)
	expect(mid.y < left.y and near(left.y, right.y) and near(left.h, right.h)).to_be(true)
	expect(left.y + left.h < after.y).to_be(true)
	-- (Never read, a URL and a home path stay the links written, by their alt.)
	expect(app:has('~"Never read: a URL and home."')).to_be(true)
end)

test("pictures together share one tray: one row, as tall, ending on its edge", function()
	local app = open()
	local a = boxes(app, 'button "Open The shell"')[2]
	local b = box(app, 'button "Open The transcript"')
	local c = box(app, 'button "Open Pixels"')
	expect(near(a.y, b.y) and near(b.y, c.y)).to_be(true)
	expect(near(a.h, b.h) and near(b.h, c.h)).to_be(true)
	-- 4 from each other and from the tray's edges: 660 less 8 across.
	expect(near(b.x - (a.x + a.w), 4)).to_be(true)
	expect(near(c.x - (b.x + b.w), 4)).to_be(true)
	expect(near(c.x + c.w - a.x, 652)).to_be(true)
	expect(app:has('"The second is the one to look at."')).to_be(true)
end)

test("a long strip shows its middle, a long page its top, an icon its own size", function()
	local app = open()
	-- 2400 by 360: five times as wide as tall, at the column's width.
	local wide = box(app, 'button "Open Frame times"')
	expect(near(wide.w, 660)).to_be(true)
	expect(near(wide.h, 132)).to_be(true)
	-- 520 by 1800: half as wide as tall, 420 tall.
	local tall = box(app, 'button "Open The whole page"')
	expect(near(tall.w, 210)).to_be(true)
	expect(near(tall.h, 420)).to_be(true)
	-- 24 by 24: a 64-point tile.
	local icon = box(app, 'button "Open The icon"')
	expect(near(icon.w, 64) and near(icon.h, 64)).to_be(true)
end)

test("one still coming holds the column at 16:10", function()
	local app = open()
	local coming = box(app, '"The build log, loading"')
	expect(near(coming.w, 660)).to_be(true)
	expect(near(coming.h, 412.5)).to_be(true)
end)

test("one that can't be read is a quiet line; its reason is never drawn in the reply", function()
	local app = open()
	expect(app:has('"Settings couldn\'t be shown"')).to_be(true)
	expect(app:has('text "No such file in the chat\'s folder"')).to_be(false)
	expect(app:has('"Open Settings"')).to_be(false)
end)

test("a URL and a home path are never read; a GIF shows at its own size", function()
	local app = open()
	local reads = app:state("fixture_reads") or ""
	expect(reads:find("https://", 1, true)).to_be_nil()
	expect(reads:find("~/", 1, true)).to_be_nil()
	-- 8 by 8: a 64-point tile, its frame at its own size.
	local gif = box(app, 'button "Open The spinner"')
	expect(near(gif.w, 64) and near(gif.h, 64)).to_be(true)
end)

test("pictures an agent shows sit in the reply where shown: two on a tray, one alone", function()
	local app = open()
	local a = box(app, 'button "Open icon-a.png"')
	local b = box(app, 'button "Open icon-b.png"')
	expect(near(a.y, b.y) and near(a.h, b.h)).to_be(true)
	local before = box(app, '"Two directions:"')
	local after = box(app, '"The first reads better small."')
	expect(before.y < a.y and a.y + a.h < after.y).to_be(true)
	-- 868 by 617 alone: 420 tall.
	local one = box(app, 'button "Open shell.png"')
	expect(near(one.h, 420)).to_be(true)
	expect(near(one.w, 868 * 420 / 617)).to_be(true)
end)

test("a failed display call is a line in plain words, nothing raw", function()
	local app = open()
	expect(app:has('"Couldn\'t show this image"')).to_be(true)
	expect(app:has('"Couldn\'t show this visual"')).to_be(true)
	expect(app:has('~"attempt to index"')).to_be(false)
	expect(app:has('~"visual:2"')).to_be(false)
end)

test("a picture opens in the lightbox among its tray's", function()
	local app = open()
	app:click('role:button "Open Pixels"')
	app:frames(60)
	expect(app:has('"3 of 3"')).to_be(true)
	app:key("Escape")
	app:frames(60)
	expect(app:state("ds/lightbox.luau::open")).to_be_nil()
end)

test("a picture already shown isn't built again as the reply is written", function()
	local app = open()
	-- (The fixture writes the reply three letters every 40 ms, on the test's
	-- clock: its frames.)
	app:command("stream")
	local first: any = nil
	for _ = 1, 200 do
		app:frames(1)
		if app:tree():find('button "Open The live shell"', 1, true) then
			first = box(app, 'button "Open The live shell"')
			break
		end
	end
	expect(first ~= nil).to_be(true)
	-- On to the reply's end: the words after the picture written under it.
	for _ = 1, 400 do
		app:frames(1)
		if app:has('"And that\'s the whole of it, the sidebar at its own width."') then
			break
		end
	end
	expect(app:has('"And that\'s the whole of it, the sidebar at its own width."')).to_be(true)
	local again = box(app, 'button "Open The live shell"')
	expect(again.id).to_be(first.id)
	expect(near(again.h, first.h)).to_be(true)
end)

test("the gallery's Reply pictures page mounts with every state", function()
	local app = mount("reply_pictures_gallery_fixture.luau")
	app:resize(1000, 3000)
	app:frames(30)
	expect(app:error()).to_be_nil()
	expect(app:has('"Open Pixels"')).to_be(true)
	expect(app:has('"The build log, loading"')).to_be(true)
	expect(app:has('"Settings couldn\'t be shown"')).to_be(true)
	expect(app:has('"Couldn\'t show this visual"')).to_be(true)
end)

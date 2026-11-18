-- A tool whose result carried a picture (a Read of a screenshot: the
-- engine's copy, `previewPaths`) opens to it as a small thumbnail, its own
-- shape within the theme's 240 by 160 (`zeron.prose.picture.thumb`), never
-- the reading column's width; a click opens it in the lightbox. Several sit
-- in a row. A Read that kept none opens to nothing of the kind
-- (ui/tool_pictures_fixture.luau).

local function work(): any
	local app = mount("tool_pictures_fixture.luau")
	app:resize(900, 900)
	app:frames(10)
	expect(app:error()).to_be_nil()
	-- (Settled work folds to its line: open it.)
	app:click('role:button "Read 2 files · ran 1 command"')
	app:frames(30)
	return app
end

-- A node's place and size in the tree, by a pattern on its line.
local function box(app: any, pattern: string): (number?, number?, number?, number?)
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			local x, y, w, h = line:match("(%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
			if x then
				return tonumber(x), tonumber(y), tonumber(w), tonumber(h)
			end
		end
	end
	return nil
end

test("a read whose result kept a picture opens to a thumbnail of it, and it opens in the lightbox", function()
	local app = work()
	expect(app:has('"Open test.png"')).to_be(false)
	app:click('role:button "Read test.png"')
	app:frames(30)
	expect(app:error()).to_be_nil()
	expect(app:has('"Open test.png"')).to_be(true)
	-- (868 by 617: as tall as the thumbnail allows, its shape kept.)
	local _, _, w, h = box(app, '"Open test.png"')
	expect(w).never.to_be_nil()
	expect(h <= 160.5).to_be(true)
	expect(w <= 240.5).to_be(true)
	expect(math.abs(w / h - 868 / 617) < 0.02).to_be(true)
	app:click('"Open test.png"')
	app:frames(60)
	local open = app:state("ds/lightbox.luau::open")
	expect(open).never.to_be_nil()
	expect(open.items[open.index].src).to_be("assets/composer/shell.png")
end)

test("several pictures are thumbnails side by side", function()
	local app = work()
	app:click('role:button "Ran shoot both"')
	app:frames(30)
	expect(app:error()).to_be_nil()
	local lx, ly, lw, lh = box(app, '"Open left.png"')
	local rx, ry = box(app, '"Open right.png"')
	expect(lx).never.to_be_nil()
	expect(rx).never.to_be_nil()
	expect(ly).to_be(ry)
	expect(rx > lx + lw - 0.5).to_be(true)
	expect(lh <= 160.5).to_be(true)
end)

test("a read that kept no picture shows none", function()
	local app = work()
	app:click('role:button "Read old.png"')
	app:frames(30)
	expect(app:error()).to_be_nil()
	expect(app:has('"Open old.png"')).to_be(false)
end)

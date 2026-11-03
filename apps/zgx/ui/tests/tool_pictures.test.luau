-- A tool whose result carried a picture (a Read of a screenshot: the
-- engine's copy, `previewPaths`) opens to that picture, as Zeron's chip
-- previews it; a click opens it in the lightbox. A Read that kept none
-- opens to nothing of the kind (ui/tool_pictures_fixture.luau).

local function work(): any
	local app = mount("tool_pictures_fixture.luau")
	app:resize(900, 900)
	app:frames(10)
	expect(app:error()).to_be_nil()
	-- (Settled work folds to its line: open it.)
	app:click('role:button "Read 2 files"')
	app:frames(30)
	return app
end

test("a read whose result kept a picture opens to it, and it opens in the lightbox", function()
	local app = work()
	expect(app:has('"Open test.png"')).to_be(false)
	app:click('role:button "Read test.png"')
	app:frames(30)
	expect(app:error()).to_be_nil()
	expect(app:has('"Open test.png"')).to_be(true)
	app:click('"Open test.png"')
	app:frames(60)
	local open = app:state("ds/lightbox.luau::open")
	expect(open).never.to_be_nil()
	expect(open.items[open.index].src).to_be("assets/composer/shell.png")
end)

test("a read that kept no picture shows none", function()
	local app = work()
	app:click('role:button "Read old.png"')
	app:frames(30)
	expect(app:error()).to_be_nil()
	expect(app:has('"Open old.png"')).to_be(false)
end)

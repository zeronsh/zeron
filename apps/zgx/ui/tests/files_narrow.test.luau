-- A narrow files pane (the panel's Files tab, under the tree's 288 and a
-- file's least 240 beside it: 528) never shares its row: a file opened shows
-- at the pane's whole width, the tree closed; the tree opened (its toggle)
-- covers the whole pane, and a file chosen there goes back to the file.
-- Wider, the tree stands beside the file as it always has. Crossing the
-- width keeps the file shown; widening brings the tree back beside it if it
-- was out. On the pane in ui/files_fixture.luau.

local TREE = 288

local function settle(app: any)
	app:frames(60)
end

-- A node's left edge and width, by its target ("" if it isn't there).
local function box(app: any, target: string): (number, number)
	local dump: string = app:tree(target)
	local line: string = string.match(dump, "[^\n]+") or ""
	local x, w = string.match(line, " (%-?%d+),%-?%d+ (%d+)x%d+")
	return tonumber(x) or 0, tonumber(w) or 0
end

local function width(app: any, target: string): number
	local _, w = box(app, target)
	return w
end

-- Whether the tree is out (any of it in the pane) and how wide it shows.
local function tree_out(app: any): (boolean, number)
	local x, w = box(app, "#files-tree-beside")
	return x + w > 1, x + w
end

local function open(w: number): any
	local app = mount("files_fixture.luau")
	app:resize(w, 700)
	settle(app)
	expect(app:error()).to_be_nil()
	expect(app:wait('role:treeitem "notes.md"', { timeout_ms = 15000 })).to_be(true)
	return app
end

local function pick(app: any, name: string)
	app:click(`role:treeitem "{name}"`)
	settle(app)
end

test("a narrow pane opens a file at its whole width and closes the tree", function()
	local app = open(420)
	local pane = width(app, "#files-pane")
	expect(pane < TREE + 240).to_be(true)
	pick(app, "notes.md")
	local out = tree_out(app)
	expect(`tree out: {out}`).to_be("tree out: false")
	expect(math.abs(width(app, "#file-viewer") - pane) <= 1).to_be(true)
end)

test("a narrow pane's tree opens at the pane's whole width", function()
	local app = open(420)
	local pane = width(app, "#files-pane")
	pick(app, "notes.md")
	app:click('role:button "Toggle File Tree"')
	settle(app)
	local out, shows = tree_out(app)
	expect(out).to_be(true)
	expect(math.abs(shows - pane) <= 1).to_be(true)
	-- And closes again, the file at its whole width.
	app:click('role:button "Toggle File Tree"')
	settle(app)
	expect((tree_out(app))).to_be(false)
	expect(math.abs(width(app, "#file-viewer") - pane) <= 1).to_be(true)
end)

test("a file chosen from the narrow pane's tree goes back to the file", function()
	local app = open(420)
	local pane = width(app, "#files-pane")
	pick(app, "notes.md")
	app:click('role:button "Toggle File Tree"')
	settle(app)
	-- Another file.
	pick(app, "code.luau")
	expect(app:wait('role:textbox "code.luau"', { timeout_ms = 15000 })).to_be(true)
	expect((tree_out(app))).to_be(false)
	expect(math.abs(width(app, "#file-viewer") - pane) <= 1).to_be(true)
	-- The same file again.
	app:click('role:button "Toggle File Tree"')
	settle(app)
	pick(app, "code.luau")
	expect((tree_out(app))).to_be(false)
	expect(math.abs(width(app, "#file-viewer") - pane) <= 1).to_be(true)
end)

test("a wide pane keeps the tree beside the file, as before", function()
	local app = open(1000)
	local pane = width(app, "#files-pane")
	pick(app, "notes.md")
	local out, shows = tree_out(app)
	expect(out).to_be(true)
	expect(shows).to_be(TREE)
	expect(math.abs(width(app, "#file-viewer") - (pane - TREE)) <= 1).to_be(true)
	-- Its toggle hides it and shows it again beside the file.
	app:click('role:button "Toggle File Tree"')
	settle(app)
	expect((tree_out(app))).to_be(false)
	app:click('role:button "Toggle File Tree"')
	settle(app)
	out, shows = tree_out(app)
	expect(out).to_be(true)
	expect(shows).to_be(TREE)
end)

test("resizing across the width keeps the file, and brings the tree back beside it", function()
	local app = open(1000)
	pick(app, "notes.md")
	expect((tree_out(app))).to_be(true)
	-- Narrowed past it: the file stays, at the whole width.
	app:resize(420, 700)
	settle(app)
	local pane = width(app, "#files-pane")
	expect((tree_out(app))).to_be(false)
	expect(math.abs(width(app, "#file-viewer") - pane) <= 1).to_be(true)
	expect(app:has('#file-viewer >> "notes.md"')).to_be(true)
	-- The tree opened there, then wide again: beside the file, its own width.
	app:click('role:button "Toggle File Tree"')
	settle(app)
	app:resize(1000, 700)
	settle(app)
	local out, shows = tree_out(app)
	expect(out).to_be(true)
	expect(shows).to_be(TREE)
	expect(app:has('#file-viewer >> "notes.md"')).to_be(true)
	-- Narrow again with the tree beside: the file, at the whole width.
	app:resize(420, 700)
	settle(app)
	expect((tree_out(app))).to_be(false)
	-- A tree hidden while wide stays hidden across the width and back.
	app:resize(1000, 700)
	settle(app)
	app:click('role:button "Toggle File Tree"')
	settle(app)
	expect((tree_out(app))).to_be(false)
	app:resize(420, 700)
	settle(app)
	app:resize(1000, 700)
	settle(app)
	expect((tree_out(app))).to_be(false)
end)

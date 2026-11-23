local function mount_diff(): any
	local app = mount("git_changes_scroll_fixture.luau")
	app:resize(400, 360)
	app:frames(30)
	return app
end

local function box(app: any, selector: string): { x: number, y: number, w: number, h: number }
	local tree = app:tree(selector)
	local x, y, w, h = tree:match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
	assert(x and y and w and h, tree)
	return { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number }
end

local FIRST = '" first visible line"'
local FILE = 'role:button "src/scroll.rs"'

test("diff horizontal scrolling reaches offscreen lines and pins headers and comments", function()
	local app = mount_diff()
	local before, file, comment = box(app, FIRST), box(app, FILE), box(app, "#pinned-comment")
	-- The only very long line is below the mounted slice.
	expect(app:tree():find("END", 1, true)).to_be(nil)
	app:wheel("#scroll-diff", { dx = 600, dy = 0 })
	app:frames(10)
	expect(box(app, FIRST).x < before.x - 100).to_be(true)
	expect(box(app, FILE).x).to_be(file.x)
	expect(box(app, "#pinned-comment").x).to_be(comment.x)
	expect(box(app, "#pinned-comment").w).to_be(comment.w)
	app:scroll("#scroll-diff", 150)
	app:frames(10)
	-- Still a bounded virtual list, and the sticky file header stays put.
	expect(box(app, FILE .. "[2]").x).to_be(file.x)
	expect(app:tree():find("END", 1, true)).to_be(nil)
end)

test("diff Wrap resets horizontal scrolling and resizing clamps it", function()
	local app = mount_diff()
	local start = box(app, FIRST).x
	app:wheel("#scroll-diff", { dx = 600, dy = 0 })
	app:set_state("test_diff_wrap", true)
	app:frames(30)
	expect(box(app, FIRST).x).to_be(start)
	app:wheel("#scroll-diff", { dx = 300, dy = 0 })
	app:frames(10)
	expect(box(app, FIRST).x).to_be(start)
	app:set_state("test_diff_wrap", false)
	app:frames(20)
	app:wheel("#scroll-diff", { dx = 600, dy = 0 })
	app:resize(20000, 360)
	app:frames(30)
	expect(box(app, FIRST).x).to_be(start)
end)

test("closing or shortening a diff removes its horizontal overflow", function()
	local app = mount_diff()
	local start = box(app, FILE).x
	app:wheel("#scroll-diff", { dx = 600, dy = 0 })
	app:set_state("test_diff_open", {})
	app:frames(20)
	app:set_state("test_diff_open", { ["src/scroll.rs"] = true })
	app:frames(20)
	expect(box(app, FIRST).x).to_be(start)
	app:wheel("#scroll-diff", { dx = 600, dy = 0 })
	app:set_state("test_diff_short", true)
	app:frames(30)
	expect(box(app, '" short"').x).to_be(start)
end)

test("bare and side by side diffs also scroll horizontally", function()
	local app = mount_diff()
	app:set_state("test_diff_bare", true)
	app:set_state("test_diff_split", true)
	app:frames(30)
	local before = box(app, ".diff-pair[1]")
	app:wheel("#scroll-diff", { dx = 600, dy = 0 })
	app:frames(10)
	expect(box(app, ".diff-pair[1]").x < before.x - 100).to_be(true)
	app:set_state("test_diff_wrap", true)
	app:frames(30)
	expect(box(app, ".diff-pair[1]").x).to_be(before.x)
end)

test("diff horizontal rail moves code and native scrolling returns to the start", function()
	local app = mount_diff()
	local before = box(app, FIRST).x
	app:hover("#scroll-diff")
	app:frames(30)
	app:click("#scroll-diff-horizontal-rail")
	app:frames(10)
	expect(box(app, FIRST).x < before - 100).to_be(true)
	-- Returning to the start through the native horizontal scroll view.
	app:wheel("#scroll-diff", { dx = -20000, dy = 0 })
	app:frames(10)
	expect(box(app, FIRST).x).to_be(before)
end)

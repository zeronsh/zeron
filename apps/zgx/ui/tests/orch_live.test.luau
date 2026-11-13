-- The orchestration live pieces (ui/orch_live_fixture.luau): a status
-- shows 300 ms at least before the next takes its place (ds/orch_hold),
-- the latest winning; cells and a sparkline draw and resize.

test("orch live: a status that changes at once is held, then the latest shows", function()
	local app = mount("orch_live_fixture.luau")
	app:resize(600, 400)
	app:frames(1)
	expect(app:text("#held")).to_be("working")
	-- Two changes inside the hold: neither shows yet.
	app:set_state("kind", "failed")
	app:frames(1)
	app:set_state("kind", "done")
	app:frames(1)
	expect(app:text("#held")).to_be("working")
	-- Past it: the latest.
	expect(app:wait("#held", { text = "done", timeout_ms = 2000 })).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch live: a change back to what shows drops the one waiting", function()
	local app = mount("orch_live_fixture.luau")
	app:resize(600, 400)
	app:frames(1)
	app:set_state("kind", "failed")
	app:frames(1)
	app:set_state("kind", "working")
	app:frames(40)
	expect(app:text("#held")).to_be("working")
end)

test("orch live: cells and a sparkline draw and resize", function()
	local app = mount("orch_live_fixture.luau")
	for _, size in { { 600, 400 }, { 320, 300 }, { 1200, 600 } } do
		app:resize(size[1], size[2])
		app:set_state("done", size[1] % 9)
		app:frames(5)
		expect(app:has("#cells")).to_be(true)
		expect(app:has("#spark")).to_be(true)
		expect(app:error()).to_be_nil()
	end
end)

-- A node's box from the tree: x, y, w, h.
local function box(app: any, target: string): (number, number, number, number)
	local line = app:tree(target):match("[^\n]+") or ""
	local x, y, w, h = line:match(" (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
	return tonumber(x) or -1, tonumber(y) or -1, tonumber(w) or -1, tonumber(h) or -1
end

test("orch live: the current step's light sits on its cell, at every width and mid-pulse", function()
	local app = mount("orch_live_fixture.luau")
	for _, size in { { 600, 400 }, { 320, 300 }, { 1200, 600 } } do
		app:resize(size[1], size[2])
		for _, d in { 0, 3, 7 } do
			app:set_state("done", d)
			-- (Mid-pulse too: a few frames into its loop.)
			for _ = 1, 3 do
				app:frames(4)
				for _, id in { "cells", "cells-fill" } do
					local cx, cy, cw, ch = box(app, `#{id}`)
					local x, y, w, h = box(app, `#{id}-current`)
					expect(w > 0 and h > 0).to_be(true)
					expect(x >= cx and x + w <= cx + cw + 0.5).to_be(true)
					expect(y >= cy and y + h <= cy + ch + 0.5).to_be(true)
				end
				-- The 1-based step under way: the one after those done.
				local cx = box(app, "#cells")
				local x = box(app, "#cells-current")
				expect(math.abs((x - cx) - d * 7) < 0.6).to_be(true)
			end
		end
	end
end)

test("orch live: a short line starts at the left and grows; a full one scrolls", function()
	local app = mount("orch_live_fixture.luau")
	app:frames(1)
	local p = app:state("spark_place")
	-- Twenty seconds of a six-minute window, 300 wide: from the left edge,
	-- its head well short of the right.
	expect(p.short_full).to_be(false)
	expect(p.short_first).to_be(0)
	expect(math.abs(p.short_head - 300 * 20 / 360) < 0.01).to_be(true)
	-- The window full: the head at the right edge, the oldest past the left.
	expect(p.long_full).to_be(true)
	expect(math.abs(p.long_head - 300) < 0.01).to_be(true)
	expect(p.long_first < 0).to_be(true)
	-- No samples yet: nothing to place.
	expect(p.none_full).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orch live: a rolling figure keeps its width and never runs into its label", function()
	local app = mount("orch_live_fixture.luau")
	app:resize(600, 400)
	app:frames(2)
	local lx, _, lw = box(app, "#cpu-label")
	local _, _, w0 = box(app, "#counter")
	expect(w0 > 0).to_be(true)
	for _, v in { 33, 100, 4 } do
		app:set_state("cpu", v)
		-- Frame by frame through the roll.
		for _ = 1, 40 do
			app:frames(1)
			local x, _, w = box(app, "#counter")
			expect(math.abs(w - w0) < 0.5).to_be(true)
			expect(x >= lx + lw).to_be(true)
		end
	end
	expect(app:has('#counter >> "4"')).to_be(true)
end)

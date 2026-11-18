-- The Map on a huge fleet (ui/orch_map_fixture.luau `map_huge`: 165
-- threads): in every view it stands no taller than its bound (ds/orch_map
-- `max_h`, about the window), scrolling inside it, and no canvas of it is
-- taller than a band (ds/orch_map_kit `Tiled`), so none passes zgx's
-- 32 MiB raster limit, which would cost the window its frame: the Control
-- room froze on a 1044 × 6356 Timeline.

local VIEWS = { "timeline", "topology", "machines", "graph", "force" }
-- 4 bytes a pixel at a display scale of 3, the most the Map meets.
local LIMIT = 32 * 1024 * 1024
local SCALE = 3

local function open(w: number, h: number): any
	local app = mount("orch_map_fixture.luau")
	app:resize(w, h)
	app:command("map_only")
	app:command("map_huge")
	app:frames(20)
	return app
end

-- Each node's box (w, h) that `target` matches: the tree of one, else the
-- harness's list of them all (its error for more than one).
local function boxes(app: any, target: string): { { w: number, h: number } }
	local out = {}
	if not app:has(target) then
		return out
	end
	local ok, text = pcall(function(): string
		return app:tree(target)
	end)
	local lines = tostring(text)
	for line in string.gmatch(lines, "[^\n]+") do
		-- (Only the nodes themselves: a tree's first line, the list's each.)
		local w, h = line:match("^%s*@%d+ .-%-?[%d.]+,%-?[%d.]+ ([%d.]+)x([%d.]+)")
		if w and (not ok or #out == 0) then
			table.insert(out, { w = tonumber(w) :: number, h = tonumber(h) :: number })
		end
	end
	return out
end

for _, view in VIEWS do
	test(`{view}: a huge fleet's Map stays within its bound, its canvases under the raster limit`, function()
		local app = open(1440, 900)
		app:command(`map_{view}`)
		app:frames(40)
		expect(app:error()).to_be(nil)
		local map = boxes(app, "#control-widget-map")[1]
		expect(map ~= nil).to_be(true)
		-- (The bound, three quarters of 900, with the widget's own head and
		-- padding around it.)
		expect(map.h <= 675 + 120).to_be(true)
		for _, class in { ".orch-tl-chart", ".orch-map-lines", ".orch-force-canvas", ".orch-force-overlay", "canvas" } do
			for _, b in boxes(app, `#control-widget-map >> {class}`) do
				expect(b.w * b.h * SCALE * SCALE * 4 <= LIMIT).to_be(true)
			end
		end
	end)
end

test("timeline: every lane's band is drawn, and the Map scrolls to its last lane", function()
	local app = open(1440, 900)
	app:command("map_timeline")
	app:frames(40)
	local tiles = boxes(app, "#control-widget-map >> .orch-tl-chart")
	expect(#tiles >= 8).to_be(true)
	local total = 0
	for _, b in tiles do
		expect(b.h <= 512).to_be(true)
		total += b.h
	end
	-- (165 lanes at 26 points: past 4,000 points of field, all covered.)
	expect(total >= 165 * 26).to_be(true)
	expect(app:has('#control-widget-map >> .orch-map-scroll')).to_be(true)
end)

test("the Control room keeps drawing and answering with a huge Map", function()
	local app = open(1440, 900)
	app:command("map_timeline")
	app:frames(20)
	for i = 1, 30 do
		app:command(if i % 2 == 0 then "map_timeline" else "map_topology")
		app:frames(2)
	end
	expect(app:error()).to_be(nil)
	-- (Still answering: the menu's view switch lands.)
	app:command("map_machines")
	app:frames(10)
	expect(app:has("#control-widget-map >> .orch-map-scroll")).to_be(true)
	expect(app:error()).to_be(nil)
end)

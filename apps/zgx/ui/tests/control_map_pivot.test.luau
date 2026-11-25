-- The Graph's pivot (orchestrators or machines as its hubs) on the Control
-- room's Map: in the Map's head beside its menu, not in the field's corner,
-- so nothing floating over the page (Needs you, Capture's history and bar)
-- ever covers it; it switches the hubs from there. On
-- ui/control_page_fixture.luau (ui/control_world, whose sample has asks
-- waiting and dumps sent).

local SIZES = { { 2024, 1102 }, { 1637, 1046 }, { 1440, 900 }, { 1100, 800 } }
local PIVOT = "#orch-force-pivot"

-- A node's box (x, y, w, h), or nil.
local function box(app: any, target: string): (number?, number?, number?, number?)
	if not app:has(target) then
		return nil, nil, nil, nil
	end
	local line = string.match(app:tree(target, 0), "[^\n]+") or ""
	local x, y, w, h = string.match(line, " (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
	return tonumber(x), tonumber(y), tonumber(w), tonumber(h)
end

local function overlaps(app: any, a: string, b: string): boolean
	local ax, ay, aw, ah = box(app, a)
	local bx, by, bw, bh = box(app, b)
	if not ax or not bx then
		return false
	end
	return ax < bx + bw and bx < ax + aw and ay < by + bh and by < ay + ah
end

for _, size in SIZES do
	local w, h = size[1], size[2]
	test(`map pivot: in the Map's head, never under what floats, at {w}x{h}`, function()
		local app = mount("control_page_fixture.luau")
		app:resize(w, h)
		app:frames(30)
		app:set_state("control_tab", "map")
		app:set_state("control_needs_min", false)
		app:set_state("control_history_panel", true)
		app:wait("#no-such-node", { timeout_ms = 400 })
		app:frames(20)
		expect(app:has("#control-needs-float")).to_be(true)
		expect(app:has("#control-history")).to_be(true)
		-- One switch, in the head (on the title's line, before the menu), none
		-- in the field's corner.
		expect(app:count(PIVOT)).to_be(1)
		expect(app:has("#control-map >> .orch-force-pivot")).to_be(false)
		expect(app:has(`#control-map >> .orch-block-head >> {PIVOT}`)).to_be(true)
		local _, py, _, ph = box(app, PIVOT)
		local _, ty, _, th = box(app, "#control-map >> .orch-block-title")
		expect(math.abs((py + ph / 2) - (ty + th / 2)) <= 2).to_be(true)
		local px = box(app, PIVOT)
		local mx = box(app, "#control-map >> .control-panel-menu")
		expect(px < mx).to_be(true)
		-- Clear of all that floats.
		for _, over in { "#control-needs-float", "#control-history", "#control-float", "#control-history-open" } do
			expect(`{over} {overlaps(app, PIVOT, over)}`).to_be(`{over} false`)
		end
		-- And in reach: a click there switches the hubs.
		local was = app:state("control_map_force_pivot")
		app:click(`{PIVOT} >> role:radio "Machine hubs"`)
		app:frames(20)
		expect(app:state("control_map_force_pivot")).to_be("machine")
		app:click(`{PIVOT} >> role:radio "Orchestrator hubs"`)
		app:frames(20)
		expect(app:state("control_map_force_pivot")).to_be("orchestrator")
		expect(was ~= nil).to_be(true)
		expect(app:error()).to_be_nil()
	end)
end

test("map pivot: only while the Graph shows; the Machines view has none", function()
	local app = mount("control_page_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	expect(app:has(PIVOT)).to_be(true)
	app:set_state("control_map", { variant = "machines" })
	app:frames(30)
	expect(app:has(PIVOT)).to_be(false)
	app:set_state("control_map", { variant = "force" })
	app:frames(30)
	expect(app:has(PIVOT)).to_be(true)
	expect(app:error()).to_be_nil()
end)

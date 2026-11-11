-- A chart's readout under the pointer (ds/chart): the pointer on a bar or
-- a point says its category and values, and the chart stays one height
-- with it there or not, so the reply under it doesn't move as the pointer
-- crosses it. Every kind: a lone series across (Wing's), a lone series of
-- bars, grouped and stacked bars, a line, areas, scatter, and a legend
-- that wraps in a narrow column. (chart_fixture.luau.)

-- A node's box, from the tree's first line: `@n … x,y WxH`.
local function box(app: any, target: string): { x: number, y: number, w: number, h: number }
	local x, y, w, h = app:tree(target):match("^%s*@%d+[^\n]- ([%-%d.]+),([%-%d.]+) ([%d.]+)x([%d.]+)")
	return { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number }
end

local function only(name: string, width: number?): any
	local app = mount("chart_fixture.luau")
	app:resize(900, 900)
	app:command(`only_{name}`)
	app:command(`width_{width or 640}`)
	app:frames(6)
	expect(app:error()).to_be_nil()
	return app
end

-- The chart's height (its heading's, its own and the words' under it),
-- and where the words under it start.
local function place(app: any, name: string): { h: number, below: number }
	return {
		h = box(app, `#chart-{name}`).h,
		below = box(app, `#below-{name}`).y,
	}
end

-- The pointer onto the plot, then off it again: the chart's height and
-- what's under it never move. `says` is what the readout shows on it (a
-- value: the categories are the plot's own words too).
local function steady(name: string, says: string?, width: number?)
	local app = only(name, width)
	local rest = place(app, name)
	app:hover(`#chart-{name} >> .chart-plot`)
	app:frames(4)
	if says then
		expect(app:has(`#chart-{name} >> ~"{says}"`)).to_be(true)
	end
	local on = place(app, name)
	expect(on.h).to_be(rest.h)
	expect(on.below).to_be(rest.below)
	app:hover(`#below-{name}`)
	app:frames(4)
	if says then
		expect(app:has(`#chart-{name} >> ~"{says}"`)).to_be(false)
	end
	local off = place(app, name)
	expect(off.h).to_be(rest.h)
	expect(off.below).to_be(rest.below)
end

test("chart: a lone series across says its bar's value under the pointer, and the reply under it stays put", function()
	-- The plot's middle is the third bar of four.
	steady("across", "4.40")
end)

test("chart: a lone series of bars keeps its height under the pointer", function()
	steady("bar", "51")
end)

test("chart: grouped bars keep their height under the pointer", function()
	steady("grouped", "51")
end)

test("chart: stacked bars keep their height under the pointer", function()
	steady("stacked", "51")
end)

test("chart: a line keeps its height under the pointer", function()
	steady("line", "68")
end)

test("chart: areas keep their height under the pointer", function()
	steady("area", "168")
end)

test("chart: scatter keeps its height under the pointer", function()
	steady("scatter")
end)

test("chart: a legend that wraps in a narrow column keeps its height under the pointer", function()
	steady("crowded", "41.0", 320)
end)

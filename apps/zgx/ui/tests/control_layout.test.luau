-- The Control room's layout (screens/control/page): two columns from 1100
-- points, one below; widgets hidden and moved, kept on this device. On
-- ui/control_page_fixture.luau, and in the shell (ui/control_fixture.luau).

local function open(w: number?, h: number?): any
	local app = mount("control_page_fixture.luau")
	app:resize(w or 1440, h or 900)
	app:frames(30)
	return app
end

local function top(app: any, target: string): number?
	local line = app:tree(target):match("[^\n]+")
	return line and tonumber(line:match(" %-?%d+,(%-?%d+) "))
end
local function left(app: any, target: string): number?
	local line = app:tree(target):match("[^\n]+")
	return line and tonumber(line:match(" (%-?%d+),%-?%d+ "))
end

test("wide, two columns: Capture and the Map left; Needs you, Machines and Comms right; the Board across both", function()
	local app = open(1440, 900)
	expect(app:has("#control-left")).to_be(true)
	expect(app:has("#control-left >> #control-widget-map")).to_be(true)
	expect(app:has("#control-full >> #control-widget-board")).to_be(true)
	expect(app:has("#control-right >> #control-widget-needs")).to_be(true)
	expect(app:has("#control-right >> #control-widget-comms")).to_be(true)
	expect((left(app, "#control-widget-needs") :: number) > (left(app, "#control-widget-capture") :: number)).to_be(true)
end)

test("narrow, one column in the widgets' order", function()
	local app = open(900, 900)
	expect(app:has("#control-left")).to_be(false)
	expect(app:has(".control-narrow")).to_be(true)
	local capture, needs, map = top(app, "#control-widget-capture"), top(app, "#control-widget-needs"), top(app, "#control-widget-map")
	expect((capture :: number) < (needs :: number) and (needs :: number) < (map :: number)).to_be(true)
	-- Back to wide as the window widens, the draft kept.
	app:type("#control-capture >> role:textbox", "half a thought")
	app:resize(1500, 900)
	app:frames(20)
	expect(app:has("#control-left")).to_be(true)
	expect(app:text("#control-capture >> role:textbox")).to_be("half a thought")
end)

test("a widget hidden from its menu goes, and comes back from the page's", function()
	local app = open()
	expect(app:has("#control-widget-machines")).to_be(true)
	app:click('#control-widget-machines >> role:button "Widget menu"')
	app:frames(10)
	app:click('"Hide"')
	app:frames(20)
	expect(app:has("#control-widget-machines")).to_be(false)
	expect(app:state("control_widgets").hidden.machines).to_be(true)
	app:click("#control-widgets-menu")
	app:frames(10)
	app:click('"Machines"')
	app:frames(20)
	expect(app:has("#control-widget-machines")).to_be(true)
end)

test("a widget moved up stays in its column, and the order is kept", function()
	local app = open(1440, 2400)
	app:click('#control-widget-map >> role:button "Widget menu"')
	app:frames(10)
	app:click('"Move up"')
	app:frames(20)
	expect((top(app, "#control-widget-map") :: number) < (top(app, "#control-widget-capture") :: number)).to_be(true)
	local order = app:state("control_widgets").order
	expect(table.find(order, "map") < table.find(order, "capture")).to_be(true)
end)

test("the widgets' menu lists every widget", function()
	local app = open()
	app:click("#control-widgets-menu")
	app:frames(10)
	expect(app:has('"Machines"')).to_be(true)
	expect(app:has('"Reset"')).to_be(true)
end)

test("in the window, the Control room is a page of its own: no composer over it", function()
	local app = mount("control_fixture.luau")
	app:resize(1440, 900)
	app:frames(40)
	expect(app:has("#control-room")).to_be(true)
	expect(app:has("#orchestration-page-area")).to_be(true)
	for _, size in { { 900, 900 }, { 1440, 900 }, { 1100, 700 } } do
		app:resize(size[1], size[2])
		app:frames(10)
		expect(app:has("#control-room")).to_be(true)
	end
end)

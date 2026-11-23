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

test("wide, two columns: Capture and the Map left; Needs you, Machines and Comms right; no Board (its own tab)", function()
	local app = open(1440, 900)
	expect(app:has("#control-left")).to_be(true)
	expect(app:has("#control-left >> #control-widget-map")).to_be(true)
	expect(app:has("#control-widget-board")).to_be(false)
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
	-- (Tall enough that Machines is in view, under Needs you and Progress.)
	local app = open(1440, 2400)
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

test("each widget is a block of its own, its title and live figures in its head", function()
	local app = open(1440, 900)
	for _, id in { "capture", "needs", "map", "machines", "comms" } do
		expect(app:has(`#control-widget-{id} >> .orch-block-head`)).to_be(true)
	end
	expect(app:has('#control-widget-needs >> .orch-block-head >> ~"1 urgent"')).to_be(true)
	-- The Board's figures, over it on its tab.
	app:set_state("control_tab", "board")
	app:frames(30)
	expect(app:has('#control-board-figures >> ~"2 blocked"')).to_be(true)
end)

test("comms is a feed: newest first, a burst's time said once", function()
	local app = open(1440, 900)
	-- The latest eight, newest on top.
	expect(app:count("#control-widget-comms >> .orch-comm")).to_be(8)
	local first: string? = nil
	for line in app:tree("#control-widget-comms"):gmatch("[^\n]+") do
		local label = line:match('button "([^"]+)"')
		if label and label ~= "Widget menu" then
			first = label
			break
		end
	end
	expect(first).to_be("Coalesce wakes within 2 s")
	-- The newest five came within a minute: one time between them.
	local times = 0
	for line in app:tree("#control-widget-comms >> .orch-comms"):gmatch("[^\n]+") do
		if line:find('text "now"', 1, true) then
			times += 1
		end
	end
	expect(times).to_be(1)
end)

test("machines: each device's live line, memory, and the threads running there", function()
	local app = open(1440, 900)
	expect(app:count("#control-widget-machines >> .orch-machine")).to_be(3)
	expect(app:count("#control-widget-machines >> .orch-machine-line")).to_be(3)
	expect(app:has('#control-widget-machines >> .orch-machine-job >> ~"cargo test -p zeron-engine --lib orchestration"')).to_be(true)
	expect(app:has('#control-widget-machines >> ~"41 / 62 GB"')).to_be(true)
end)

-- A text's baseline from its box (`app:tree`'s): a centred line's stands
-- K of its size below its middle (ds/orch_block's `baseline`, 0.38).
local function baseline(app: any, target: string, size: number): number
	local line = app:tree(target):match("[^\n]+") or ""
	local y, h = line:match(" %-?[%d%.]+,(%-?[%d%.]+) [%d%.]+x([%d%.]+)")
	return (tonumber(y) or 0) + (tonumber(h) or 0) / 2 + 0.38 * size
end

test("titles and their smaller figures share a baseline", function()
	-- (The tree's boxes are whole points: a baseline is known to within one.
	-- The drawn rows measure equal to the pixel: see the shots.)
	local app = open(1440, 900)
	-- A block's head: its title (sm, 12) and its figures (xs, 11).
	local title = baseline(app, "#control-widget-machines >> .orch-block-title", 12)
	local figure = baseline(app, '#control-widget-machines >> .orch-block-figures >> "3 machines"', 11)
	expect(math.abs(title - figure) < 1).to_be(true)
	-- A machine's name and its platform after it.
	local name = baseline(app, '#control-widget-machines >> .orch-machine >> "desk"', 12)
	local platform = baseline(app, '#control-widget-machines >> .orch-machine-head "desk" >> .orch-machine-head-platform', 11)
	expect(math.abs(name - platform) < 1).to_be(true)
	-- The page's title (lg, 15) and what's going on (sm, 12).
	local head = baseline(app, '#control-head >> "Control room"', 15)
	local summary = baseline(app, "#control-summary", 12)
	-- (Three sizes apart, the nudge is past the rounding: it shows.)
	expect(math.abs(head - summary) < 0.5).to_be(true)
end)

test("the head counts what the blocks show, no glyphs between", function()
	local app = open(1440, 900)
	local head = app:tree("#control-summary")
	-- Working: the Progress block's figure; needing you: Needs you's.
	local working = string.match(app:tree("#control-widget-progress >> .orch-block-head"), '"(%d+) working"')
	expect(working ~= nil).to_be(true)
	expect(head).to_contain(`"{working} working"`)
	-- Needing you: Needs you's rows and the Q&A's answers not read yet
	-- (its block's "new" figure), counted once (data.waiting_for_you).
	-- (Needs you's rows: the board's three Needs-you cards.)
	expect(app:count("#control-widget-needs >> .orch-need")).to_be(3)
	local new = tonumber(string.match(app:tree("#control-widget-qa"), '"(%d+) new"')) or 0
	expect(new > 0).to_be(true)
	expect(head).to_contain(`"{3 + new} need you"`)
	-- Orchestrators: each of yours not archived (the Brain dump isn't one).
	expect(head).to_contain('"3 orchestrators"')
	expect(head).never.to_contain("·")
end)

-- The Control room's Scratchpad (ds/orch_scratchpad): its widget among
-- the others, Notes saved as they are typed, edits from elsewhere coming in
-- live, history put back, and the drawing (ds/orch_scratchpad_draw). On
-- ui/control_page_fixture.luau, the host's calls ui/control_world's
-- (`calls`: `set_text:<text>`, `set_shapes:<n>`, `restore:<id>`).

local function open(w: number?, h: number?): any
	local app = mount("control_page_fixture.luau")
	app:resize(w or 1440, h or 2400)
	app:frames(30)
	return app
end

local function count(hay: string, needle: string): number
	local n, at = 0, 1
	while true do
		local s, e = string.find(hay, needle, at, true)
		if not s or not e then
			return n
		end
		n += 1
		at = e + 1
	end
end

local function top(app: any, target: string): number?
	local line = app:tree(target):match("[^\n]+")
	return line and tonumber(line:match(" %-?%d+,(%-?%d+) "))
end

local function status(app: any): string
	return app:text("#control-widget-scratchpad >> .orch-scratchpad-status")
end

test("the widget shows the note, saved, and an empty one its placeholder", function()
	local app = open()
	expect(app:error()).to_be_nil()
	expect(app:has("#control-left >> #control-widget-scratchpad")).to_be(true)
	expect(app:text("#scratchpad-editor")).to_contain("# Sync, the long way round")
	expect(status(app)).to_be("Saved")
	-- Read as Markdown.
	app:click("#scratchpad-preview")
	app:frames(20)
	expect(app:has('#control-widget-scratchpad >> .orch-scratchpad-preview >> "Sync, the long way round"')).to_be(true)
	expect(app:has("#scratchpad-editor")).to_be(false)
	app:click("#scratchpad-preview")
	app:frames(10)
	-- Empty.
	app:command("fixture.scratchpad_empty")
	app:frames(10)
	expect(app:text("#scratchpad-editor")).to_be("")
	app:click('#scratchpad-tabs >> role:radio "Draw"')
	app:frames(20)
	expect(app:has('#control-widget-scratchpad >> "No drawing yet"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("typing saves once, a moment after it stops, with the latest text", function()
	local app = open()
	app:fill("#scratchpad-editor", "First")
	app:frames(10)
	app:fill("#scratchpad-editor", "First thoughts")
	app:frames(10)
	expect(count(app:state("calls"), "set_text:")).to_be(0)
	expect(status(app)).to_be("Saving…")
	app:frames(50)
	local calls = app:state("calls")
	expect(count(calls, "set_text:")).to_be(1)
	expect(calls).to_contain("set_text:First thoughts")
	-- Answered, the watch not there yet: syncing; then saved.
	expect(status(app)).to_be("Syncing")
	app:frames(30)
	expect(status(app)).to_be("Saved")
	expect(app:text("#scratchpad-editor")).to_be("First thoughts")
end)

test("leaving the editor saves at once", function()
	local app = open()
	app:fill("#scratchpad-editor", "Said before leaving")
	app:frames(2)
	app:click('#control-widget-scratchpad >> role:button "Widget menu"')
	app:frames(3)
	expect(app:state("calls")).to_contain("set_text:Said before leaving")
end)

test("an edit from elsewhere shows live, and never over unsaved words", function()
	local app = open()
	app:command("fixture.scratchpad_remote")
	app:frames(30)
	expect(app:text("#scratchpad-editor")).to_contain("From the Brain dump")
	-- With words of the person's not saved yet: theirs stay, and are saved.
	app:fill("#scratchpad-editor", "Mine")
	app:frames(2)
	app:command("fixture.scratchpad_remote")
	app:frames(25)
	expect(app:text("#scratchpad-editor")).to_be("Mine")
	app:frames(60)
	expect(app:state("calls")).to_contain("set_text:Mine")
	expect(app:text("#scratchpad-editor")).to_be("Mine")
end)

test("history puts a version back, and Undo the one before this", function()
	local app = open()
	app:click("#scratchpad-history")
	app:frames(15)
	expect(app:has('"Ask Wing about the relay deploy window"')).to_be(true)
	app:click('"Ask Wing about the relay deploy window"')
	app:frames(30)
	expect(app:state("calls")).to_contain("restore:rv-2")
	expect(app:text("#scratchpad-editor")).to_contain("Ask Wing")
	-- Undo: the version before the current one (now the sample's newest).
	app:click("#scratchpad-history")
	app:frames(15)
	app:click("#scratchpad-undo-version")
	app:frames(30)
	expect(app:state("calls")).to_contain("restore:rv-6")
	expect(app:text("#scratchpad-editor")).to_contain("# Sync, the long way round")
end)

test("the widget hides, comes back and moves in the Control room", function()
	local app = open()
	app:click('#control-widget-scratchpad >> role:button "Widget menu"')
	app:frames(10)
	app:click('"Hide"')
	app:frames(20)
	expect(app:has("#control-widget-scratchpad")).to_be(false)
	expect(app:state("control_widgets").hidden.scratchpad).to_be(true)
	app:click("#control-widgets-menu")
	app:frames(10)
	app:click('"Scratchpad"')
	app:frames(20)
	expect(app:has("#control-widget-scratchpad")).to_be(true)
	app:key("Escape")
	app:frames(10)
	-- Up, past the Map.
	expect((top(app, "#control-widget-scratchpad") :: number) > (top(app, "#control-widget-map") :: number)).to_be(true)
	app:click('#control-widget-scratchpad >> role:button "Widget menu"')
	app:frames(10)
	app:click('"Move up"')
	app:frames(20)
	expect((top(app, "#control-widget-scratchpad") :: number) < (top(app, "#control-widget-map") :: number)).to_be(true)
	local order = app:state("control_widgets").order
	expect((table.find(order, "scratchpad") :: number) < (table.find(order, "map") :: number)).to_be(true)
end)

test("expanded to its page: Notes and Draw side by side, tabs when narrow, and back", function()
	local app = open(1440, 1400)
	app:click("#scratchpad-expand")
	app:frames(30)
	expect(app:has("#scratchpad-page")).to_be(true)
	expect(app:has("#control-widget-map")).to_be(false)
	expect(app:has("#scratchpad-notes-block >> #scratchpad-editor")).to_be(true)
	expect(app:has("#scratchpad-draw-block >> .orch-scratchpad-canvas")).to_be(true)
	-- A word typed there is the widget's too.
	app:fill("#scratchpad-editor", "From the page")
	app:frames(2)
	app:resize(900, 1400)
	app:frames(30)
	expect(app:has("#scratchpad-tabs")).to_be(true)
	expect(app:has("#scratchpad-draw-block")).to_be(false)
	app:click("#scratchpad-collapse")
	app:frames(30)
	expect(app:has("#scratchpad-page")).to_be(false)
	expect(app:text("#scratchpad-editor")).to_be("From the page")
	expect(app:state("calls")).to_contain("set_text:From the page")
	expect(app:error()).to_be_nil()
end)

test("the drawing draws its shapes, at two sizes", function()
	local app = open(1440, 2400)
	app:click('#scratchpad-tabs >> role:radio "Draw"')
	app:frames(20)
	expect(app:has("#control-widget-scratchpad >> .orch-scratchpad-canvas")).to_be(true)
	expect(app:tree("#control-widget-scratchpad")).to_contain("Drawing, 8 shapes")
	app:resize(900, 1800)
	app:frames(30)
	expect(app:has("#control-widget-scratchpad >> .orch-scratchpad-canvas")).to_be(true)
	app:resize(1600, 1200)
	app:frames(30)
	expect(app:error()).to_be_nil()
end)

-- The drawing, by hand: the board's marks (`ds_scratchpad_test_marks`), a tenth
-- of its width and height apart.
local function board(): any
	local app = open(1440, 2400)
	app:command("fixture.scratchpad_empty")
	app:frames(5)
	app:set_state("ds_scratchpad_test_marks", true)
	app:click('#scratchpad-tabs >> role:radio "Draw"')
	app:frames(20)
	return app
end
local function at(i: number, j: number): string
	return `#scratchpad-at-{i}-{j}`
end
local function shapes(app: any): { any }
	return app:state("ds_scratchpad_shapes")
end
local function of_kind(app: any, kind: string): any
	for _, s in shapes(app) do
		if s.kind == kind then
			return s
		end
	end
	return nil
end

test("tool keys pick the tools, and never while a field has the keys", function()
	local app = board()
	app:click(at(5, 5))
	app:frames(2)
	for _, pair in { { "p", "pen" }, { "r", "rect" }, { "o", "ellipse" }, { "a", "arrow" }, { "t", "text" }, { "v", "select" } } do
		app:key(pair[1])
		app:frames(1)
		expect(app:state("ds_scratchpad_tool")).to_be(pair[2])
	end
	-- (Typing in Notes is typing.)
	app:click('#scratchpad-tabs >> role:radio "Notes"')
	app:frames(10)
	app:fill("#scratchpad-editor", "pr")
	app:frames(2)
	expect(app:state("ds_scratchpad_tool")).to_be("select")
end)

test("a stroke and a box drawn, the box moved, deleted, undone and redone", function()
	local app = board()
	app:click(at(5, 5))
	app:key("p")
	app:drag(at(1, 2), at(4, 8))
	app:frames(2)
	expect(of_kind(app, "stroke") ~= nil).to_be(true)
	app:key("r")
	app:drag(at(5, 2), at(8, 5))
	app:frames(2)
	local box = of_kind(app, "rect")
	expect(box ~= nil).to_be(true)
	expect(box.w > 100).to_be(true)
	-- (After a box, the select tool, the box chosen.)
	expect(app:state("ds_scratchpad_tool")).to_be("select")
	local y0 = box.y
	app:drag(at(6, 3), at(6, 7))
	app:frames(2)
	box = of_kind(app, "rect")
	expect(box.y > y0 + 50).to_be(true)
	app:key("Delete")
	app:frames(2)
	expect(of_kind(app, "rect")).to_be_nil()
	expect(#shapes(app)).to_be(1)
	app:key("cmd+z")
	app:frames(2)
	box = of_kind(app, "rect")
	expect(box ~= nil and box.y > y0 + 50).to_be(true)
	app:key("cmd+shift+z")
	app:frames(2)
	expect(of_kind(app, "rect")).to_be_nil()
	app:click("#scratchpad-undo")
	app:frames(2)
	expect(of_kind(app, "rect") ~= nil).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("the drawing saves once after the last stroke, its line simplified", function()
	local app = board()
	app:click(at(5, 5))
	app:key("p")
	app:drag(at(1, 2), at(4, 8))
	app:frames(20)
	expect(count(app:state("calls"), "set_shapes:")).to_be(0)
	app:frames(50)
	local calls = app:state("calls")
	expect(count(calls, "set_shapes:")).to_be(1)
	expect(calls).to_contain("set_shapes:1")
	local sent = app:state("scratchpad_sent")
	expect(sent[1].kind).to_be("stroke")
	-- The drag's ten moves (a jog, then a straight run) down to their
	-- corners.
	expect(#sent[1].points >= 2 and #sent[1].points <= 4).to_be(true)
	app:frames(30)
	expect(status(app)).to_be("Saved")
end)

test("a drawing changed elsewhere shows live", function()
	local app = board()
	app:command("fixture.scratchpad_remote_shapes")
	app:frames(30)
	local found = false
	for _, s in shapes(app) do
		found = found or s.id == "sh-remote"
	end
	expect(found).to_be(true)
	expect(app:tree("#control-widget-scratchpad")).to_contain("Drawing, 1 shape")
end)

test("a text label written on the board", function()
	local app = board()
	app:click(at(5, 5))
	app:key("t")
	app:click(at(3, 3))
	app:frames(5)
	expect(app:has("#scratchpad-words")).to_be(true)
	app:type("#scratchpad-words", "Edge")
	app:key("Enter")
	app:frames(5)
	local t = of_kind(app, "text")
	expect(t ~= nil and t.text == "Edge").to_be(true)
	expect(app:has("#scratchpad-words")).to_be(false)
end)

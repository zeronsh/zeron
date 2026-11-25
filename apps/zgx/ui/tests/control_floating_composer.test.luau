-- Capture's bar, at the foot of every tab of the Control room
-- (screens/control/page `FloatingCapture`): a chat's composer, its chips
-- over the draft and its model under it, with no step to open it out;
-- grown, it lies over the page, which keeps its layout; typing and Enter
-- send a dump from it; ⌘⇧K and `/` give it the keys.

local DRAFT = "chat_lane_draft_orc-brain"

-- (⌘⇧K is the shell's command `control.focus_capture`: a test host runs a
-- command's key through the menu bar it doesn't have, so it's run by name.)
local function open(fixture: string?): any
	local app = mount(fixture or "control_page_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

-- Whether the bar's field holds the keys.
local function focused(app: any): boolean
	for line in app:tree("#control-float [1]"):gmatch("[^\n]+") do
		if line:find("textbox", 1, true) and line:find(" focused", 1, true) then
			return true
		end
	end
	return false
end

-- Whether it shows: kept built while the page is.
local function shows(app: any): boolean
	local line = string.match(app:tree("#control-float-layer", 0), "[^\n]+") or ""
	return not string.find(line, " hidden", 1, true)
end

test("Capture's bar: on every tab, the draft kept from one to the next", function()
	local app = open()
	expect(shows(app)).to_be(true)
	app:type("#control-float >> role:textbox", "half a thought")
	app:frames(5)
	for _, t in { "map", "chat", "notes", "board" } do
		app:set_state("control_tab", t)
		app:frames(30)
		expect(shows(app)).to_be(true)
		expect(app:text("#control-float >> role:textbox")).to_be("half a thought")
	end
	-- (Its model under it, as a chat's composer has.)
	expect(app:has('#control-float >> role:button "Model"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("Capture's bar: typed in, sent with Enter, a dump", function()
	local app = open()
	app:type("#control-float >> role:textbox", "ship the bar")
	app:frames(5)
	expect(app:state(DRAFT)).to_be("ship the bar")
	app:key("Enter")
	app:frames(5)
	expect(calls(app)).to_contain("dump:ship the bar")
	expect(app:text("#control-float >> role:textbox")).to_be("")
	expect(app:state(DRAFT)).to_be("")
	-- Still there.
	expect(shows(app)).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("Capture's bar: its model and what it carries show at once, no step to open it out", function()
	local app = open()
	expect(app:has("#control-float-knob")).to_be(false)
	expect(app:has('#control-float >> role:button "Model"')).to_be(true)
	app:command("fixture.carry")
	app:frames(20)
	-- Its chips over the draft, in the composer itself.
	expect(app:has('#control-float >> ~"notes.md"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("Capture's bar: grown, it lies over the Map, which keeps its height", function()
	local app = open()
	app:set_state("control_tab", "map")
	app:frames(60)
	local fill = app:state("orch_map_fill_h")
	expect(fill > 0).to_be(true)
	local rest = app:state("control_capture_float_h")
	app:type("#control-float >> role:textbox", "one\ntwo\nthree\nfour\nfive\nsix")
	app:command("fixture.carry")
	app:frames(60)
	-- Grown by its lines and chips…
	expect(app:state("control_capture_float_h") > rest + 40).to_be(true)
	-- …the Map's room the same: nothing laid out again under it.
	expect(app:state("orch_map_fill_h")).to_be(fill)
	expect(app:error()).to_be_nil()
end)

test("Capture's bar: ⌘⇧K and / give it the keys", function()
	local app = open("control_fixture.luau")
	app:frames(30)
	app:command("control.focus_capture")
	app:frames(10)
	expect(focused(app)).to_be(true)
	app:type("#control-float >> role:textbox", "k")
	app:frames(5)
	expect(app:state(DRAFT)).to_be("k")
	-- Escape gives them back to the window; `/` with nothing holding them.
	app:key("Escape")
	app:frames(5)
	expect(focused(app)).to_be(false)
	app:key("/")
	app:frames(10)
	expect(focused(app)).to_be(true)
	-- (Taken, not typed.)
	expect(app:state(DRAFT)).to_be("k")
	-- (No error check: the shell's fixture stands in for no `appearance`
	-- host action, its own error.)
end)

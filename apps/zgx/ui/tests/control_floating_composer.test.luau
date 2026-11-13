-- The Control room's composer floating at the page's foot while Capture is
-- scrolled up out of view (screens/control/page `FloatingCapture`): one
-- composer in two places, its draft the same in both; bare (one line and a
-- knob), opened out on a click, folded by Escape; typing and Enter send a
-- dump from it; ⌘⇧K and `/` give it the keys.

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

-- Which placement holds the keys: "float", "inline" or "" (the tree's
-- whole dump stops short of the page's depth: each placement's own).
local function focused(app: any): string
	for _, at in { { "#control-float [1]", "float" }, { "#control-capture-inline", "inline" } } do
		if app:has(at[1]) then
			for line in app:tree(at[1]):gmatch("[^\n]+") do
				if line:find("textbox", 1, true) and line:find(" focused", 1, true) then
					return at[2]
				end
			end
		end
	end
	return ""
end

local function down(app: any)
	app:scroll("#control-scroll", 900)
	app:frames(30)
end

local function up(app: any)
	app:scroll("#control-scroll", -2000)
	app:frames(30)
end

test("floating composer: Capture scrolled out of view floats it, scrolled back it goes, the draft kept", function()
	local app = open()
	expect(app:has("#control-float")).to_be(false)
	app:type("#control-capture-composer >> role:textbox", "half a thought")
	app:frames(5)
	down(app)
	expect(app:state("control_capture_floating")).to_be(true)
	expect(app:has("#control-float")).to_be(true)
	-- (Bare: one line, no model under it.)
	expect(app:text("#control-float >> role:textbox")).to_be("half a thought")
	expect(app:state("control_capture_float_open")).to_be(false)
	up(app)
	expect(app:state("control_capture_floating")).to_be(false)
	-- (Its exit played out.)
	app:frames(30)
	expect(app:has("#control-float")).to_be(false)
	expect(app:text("#control-capture-composer >> role:textbox")).to_be("half a thought")
	expect(app:error()).to_be_nil()
end)

test("floating composer: typed in while floating, sent with Enter, and the same draft in Capture", function()
	local app = open()
	down(app)
	app:type("#control-float >> role:textbox", "ship the floating composer")
	app:frames(5)
	-- One draft: Capture's, out of view, has it too.
	expect(app:state(DRAFT)).to_be("ship the floating composer")
	app:key("Enter")
	app:frames(5)
	expect(calls(app)).to_contain("dump:ship the floating composer")
	expect(app:text("#control-float >> role:textbox")).to_be("")
	expect(app:state(DRAFT)).to_be("")
	-- Still floating, still bare.
	expect(app:has("#control-float")).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("floating composer: the knob opens it out, Escape folds it back, the draft kept", function()
	local app = open()
	down(app)
	app:type("#control-float >> role:textbox", "a longer note")
	app:frames(5)
	app:click("#control-float-knob")
	app:frames(30)
	expect(app:state("control_capture_float_open")).to_be(true)
	-- Opened out: the model under it, the draft the same, the keys in it.
	expect(app:has('#control-float >> role:button "Model"')).to_be(true)
	expect(app:text("#control-float >> role:textbox")).to_be("a longer note")
	expect(focused(app)).to_be("float")
	app:key("Escape")
	app:frames(30)
	expect(app:state("control_capture_float_open")).to_be(false)
	expect(app:text("#control-float >> role:textbox")).to_be("a longer note")
	-- (Bare again: no model under it.)
	expect(app:has('#control-float >> role:button "Model"')).to_be(false)
	-- And once more, the knob folding it.
	app:click("#control-float-knob")
	app:frames(30)
	app:click("#control-float-knob")
	app:frames(30)
	expect(app:state("control_capture_float_open")).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("floating composer: the knob counts what the draft carries", function()
	local app = open()
	down(app)
	expect(app:has("#control-float >> .control-float-count")).to_be(false)
	app:command("fixture.carry")
	app:frames(20)
	expect(app:text("#control-float >> .control-float-count")).to_be("2")
	-- Opened out, they're chips over the draft.
	app:click("#control-float-knob")
	app:frames(30)
	expect(app:has('#control-float >> ~"notes.md"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("floating composer: typing in it keeps the keys as Capture comes back", function()
	local app = open()
	down(app)
	app:type("#control-float >> role:textbox", "mid sentence")
	app:frames(5)
	up(app)
	app:frames(10)
	expect(app:has("#control-float")).to_be(false)
	expect(focused(app)).to_be("inline")
	-- (Where it was: Capture in view.)
	expect(app:state("control_scroll")).to_be(0)
	expect(app:state(DRAFT)).to_be("mid sentence")
	-- And Capture's keys go with it as it floats again.
	down(app)
	app:frames(10)
	expect(app:has("#control-float")).to_be(true)
	expect(focused(app)).to_be("float")
end)

test("floating composer: ⌘⇧K and / give the composer the keys where it shows", function()
	local app = open("control_fixture.luau")
	app:frames(30)
	-- In Capture, in view.
	app:command("control.focus_capture")
	app:frames(10)
	expect(focused(app)).to_be("inline")
	-- Out of view: the keys go with it into the floating one; Escape gives
	-- them back to the window, and ⌘⇧K into the floating one again.
	down(app)
	expect(app:has("#control-float")).to_be(true)
	expect(focused(app)).to_be("float")
	app:key("Escape")
	app:frames(5)
	expect(focused(app)).to_be("")
	app:command("control.focus_capture")
	app:frames(10)
	expect(app:state("control_capture_floating")).to_be(true)
	expect(focused(app)).to_be("float")
	app:type("#control-float >> role:textbox", "k")
	app:frames(5)
	expect(app:state(DRAFT)).to_be("k")
	-- `/` with nothing holding the keys (Escape gave them back).
	app:key("Escape")
	app:frames(5)
	expect(focused(app)).to_be("")
	app:key("/")
	app:frames(10)
	expect(focused(app)).to_be("float")
	-- (Taken, not typed.)
	expect(app:state(DRAFT)).to_be("k")
	-- (No error check: the shell's fixture stands in for no `appearance`
	-- host action, its own error.)
end)

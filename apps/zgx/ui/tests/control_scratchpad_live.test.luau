-- The Scratchpad as the person uses it: keys typed into Notes (not a
-- fill), saved as they go; an engine (or app) without the Scratchpad said
-- plainly, typing still kept and saved once it has it. On
-- ui/control_page_fixture.luau (ui/control_world's stubs: `calls`).

local function open(): any
	local app = mount("control_page_fixture.luau")
	app:resize(1440, 2400)
	app:frames(30)
	return app
end

local function status(app: any): string
	return app:text("#control-widget-scratchpad >> .orch-scratchpad-status")
end

local function editor_focused(app: any): boolean
	local line: string = tostring(app:tree("#scratchpad-editor"):match("[^\n]+") or "")
	return string.find(line, " focused", 1, true) ~= nil
end

test("scratchpad: a key typed into the focused notes goes in, and is saved", function()
	local app = open()
	app:click("#scratchpad-editor")
	app:frames(5)
	expect(editor_focused(app)).to_be(true)
	local before = app:text("#scratchpad-editor")
	app:type("#scratchpad-editor", "Z")
	app:frames(5)
	local text = app:text("#scratchpad-editor")
	expect(#text).to_be(#before + 1)
	expect(text).to_contain("Z")
	expect(status(app)).to_be("Saving…")
	app:frames(60)
	expect(app:state("calls")).to_contain("set_text:")
	expect(app:state("calls")).to_contain("Z")
	app:frames(30)
	expect(status(app)).to_be("Saved")
	-- An edit within the text: in, and saved too.
	app:key("Backspace")
	app:frames(60)
	expect(app:text("#scratchpad-editor")).never.to_contain("Z")
	expect(app:error()).to_be_nil()
end)

test("scratchpad: an engine without it is said plainly, typing kept and saved once it has it", function()
	local app = open()
	expect(app:has("#scratchpad-notice")).to_be(false)
	app:command("fixture.scratchpad_no_engine")
	app:frames(10)
	expect(app:text("#scratchpad-notice")).to_be("Scratchpad needs the engine update; your text isn't saved.")
	-- Still typed into.
	app:click("#scratchpad-editor")
	app:type("#scratchpad-editor", "kept here")
	app:frames(60)
	expect(app:text("#scratchpad-editor")).to_contain("kept here")
	expect(status(app)).to_be("Not saved")
	-- The engine with it after all: the notice goes, the words are saved.
	app:command("fixture.scratchpad_engine_back")
	app:frames(10)
	expect(app:has("#scratchpad-notice")).to_be(false)
	app:frames(40)
	expect(app:state("calls")).to_contain("kept here")
	expect(app:text("#scratchpad-editor")).to_contain("kept here")
	app:frames(30)
	expect(status(app)).to_be("Saved")
	expect(app:error()).to_be_nil()
end)

test("scratchpad: an app without it is said too", function()
	local app = open()
	app:command("fixture.scratchpad_no_host")
	app:frames(10)
	expect(app:text("#scratchpad-notice")).to_be("Scratchpad needs the app update; your text isn't saved.")
	app:click("#scratchpad-editor")
	app:type("#scratchpad-editor", "x")
	app:frames(5)
	expect(app:text("#scratchpad-editor")).to_contain("x")
	expect(app:error()).to_be_nil()
end)

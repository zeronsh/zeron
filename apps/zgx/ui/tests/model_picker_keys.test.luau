-- The model picker from the keys, as Zeron's compact picker (#471): folded,
-- ←/→ and Home/End step the reasoning (the panel's search has the keys, so
-- the slider never had them there); ⌘⇧F stars the highlighted model, or
-- takes its star, without picking it. On the picker in
-- ui/picker_fixture.luau.

local function settle(app: any)
	app:frames(30)
end

local function effort(app: any): string?
	return app:state("test_picker_effort")
end

local function starred(app: any): { [string]: boolean }
	return (app:state("test_picker_favorites") or {}) :: { [string]: boolean }
end

test("folded, the arrows step the reasoning; cmd shift F stars the highlighted model", function()
	local app = mount("picker_fixture.luau")
	app:resize(700, 700)
	app:set_state("test_picker_model", "gpt")
	settle(app)
	app:click('role:button "Model"')
	settle(app)
	expect(effort(app)).to_be("medium")

	app:key("Home")
	settle(app)
	expect(effort(app)).to_be("minimal")
	app:key("ArrowRight")
	app:key("ArrowRight")
	settle(app)
	expect(effort(app)).to_be("medium")
	app:key("End")
	app:key("ArrowRight")
	settle(app)
	expect(effort(app)).to_be("xhigh")
	expect(app:wait('"Extra high"', { timeout_ms = 2000 })).to_be(true)
	app:key("ArrowLeft")
	settle(app)
	expect(effort(app)).to_be("high")
	-- Still open, still folded (the search out of sight).
	expect(app:has('"REASONING"')).to_be(true)
	expect(app:tree():find('textbox "Search"[^\n]* hidden') ~= nil).to_be(true)

	-- Opened out, ↓ moves the highlight to the next model; ⌘⇧F stars it
	-- and the chosen model stays.
	app:key("ArrowDown")
	settle(app)
	app:key("ArrowDown")
	settle(app)
	app:key("cmd+shift+f")
	settle(app)
	expect(starred(app).mini).to_be(true)
	expect(app:state("test_picker_model")).to_be("gpt")
	-- Opened out, the arrows are the search's: the reasoning stays.
	app:key("Home")
	settle(app)
	expect(effort(app)).to_be("high")
	-- Again: the star comes off.
	app:key("cmd+shift+f")
	settle(app)
	expect(starred(app).mini).to_be(nil)
	expect(app:error()).to_be(nil)
end)

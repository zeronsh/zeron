-- The model picker's motion, on the picker in ui/picker_fixture.luau: its
-- panel is never see-through (opening, opening out, folding back, following
-- its trigger), opening out and folding back change its size once (the
-- panel is a popup window: a size eased over frames resized it on every
-- one), Escape folds it back from whatever in it has the keys, and the
-- reasoning thumb is where the pointer is, not springing after it.

local function settle(app: any)
	app:frames(30)
end

-- A node's box (x, y, w, h), from its tree line.
local function box(app: any, target: string): (number, number, number, number)
	local line = app:tree(target):match("[^\n]+") or ""
	local x, y, w, h = line:match("(%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
	return tonumber(x) or -1, tonumber(y) or -1, tonumber(w) or -1, tonumber(h) or -1
end
local function height(app: any): number
	local _, _, _, h = box(app, "role:dialog")
	return h
end
local function alpha(app: any): { panel: number, fold: number }
	return app:state("test_picker_alpha") :: any
end
local function watch(app: any)
	app:set_state("test_picker_alpha", { panel = 1, fold = 0 })
end
-- Whether the panel shows (not only laid out, hidden, as a closed one is).
local function listed(app: any): boolean
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(" dialog ", 1, true) and not line:find(" hidden", 1, true) then
			return true
		end
	end
	return false
end

local function opened(app: any)
	app:resize(600, 700)
	settle(app)
	watch(app)
	app:click('role:button "Model"')
	settle(app)
end

test("the picker opens, opens out and folds back solid, its size changing once", function()
	local app = mount("picker_fixture.luau")
	opened(app)
	local folded_h = height(app)
	expect(alpha(app).panel).to_be(1)

	-- Opened out: its whole size the first frame, nothing in it fading in.
	app:click(".folded")
	app:frames(1)
	local out_h = height(app)
	expect(out_h > folded_h).to_be(true)
	settle(app)
	expect(height(app)).to_be(out_h)
	expect(alpha(app).panel).to_be(1)
	expect(alpha(app).fold).to_be(0)

	-- Folded back (Escape): its folded size the first frame.
	app:key("Escape")
	app:frames(1)
	expect(height(app)).to_be(folded_h)
	settle(app)
	expect(height(app)).to_be(folded_h)
	expect(alpha(app).panel).to_be(1)
	expect(alpha(app).fold).to_be(0)
	expect(listed(app)).to_be(true)
end)

test("the panel stays solid as its trigger moves", function()
	local app = mount("picker_fixture.luau")
	opened(app)
	app:click(".folded")
	settle(app)
	watch(app)
	local _, y0 = box(app, "role:dialog")
	app:set_state("test_picker_above", 80)
	settle(app)
	local _, y1 = box(app, "role:dialog")
	expect(y1 - y0).to_be(80)
	app:set_state("test_picker_above", 0)
	settle(app)
	expect(alpha(app).panel).to_be(1)
	expect(alpha(app).fold).to_be(0)
end)

-- Escape folds an opened-out picker back whatever in it has the keys (it
-- reached the popover from the slider or a tab, which closed it).
test("Escape folds the opened-out picker back from the slider and the tabs", function()
	local app = mount("picker_fixture.luau")
	opened(app)
	local folded_h = height(app)
	for _, target in { 'role:group "REASONING"', 'role:tab "Codex"' } do
		app:click(".folded")
		settle(app)
		app:click(target)
		settle(app)
		app:key("Escape")
		settle(app)
		expect(`{target}: open {listed(app)}`).to_be(`{target}: open true`)
		-- Folded on the chosen model, in its agent's tab.
		expect(`{target}: {height(app)}`).to_be(`{target}: {folded_h}`)
	end
	-- Folded, Escape closes it.
	app:key("Escape")
	settle(app)
	expect(listed(app)).to_be(false)
end)

-- A press on the chip of an open picker closes it (it put it back folded,
-- the tab and search lost, as if opened anew).
test("the chip of an open picker closes it rather than folding it", function()
	local app = mount("picker_fixture.luau")
	opened(app)
	app:click(".folded")
	settle(app)
	app:click('role:button "Model"')
	settle(app)
	expect(listed(app)).to_be(false)
	-- And opens it again, folded.
	app:click('role:button "Model"')
	settle(app)
	expect(listed(app)).to_be(true)
end)

-- Dragged, the thumb is where the pointer is (set as it moves, not sprung
-- after the level), and the value is the level it is let go over, set as
-- it is let go.
test("the reasoning thumb follows the pointer, the frame it moves", function()
	local app = mount("picker_fixture.luau")
	app:set_state("test_picker_effort", "low")
	opened(app)
	-- Where the thumb sits (its left, along the groove) at a fraction of
	-- the way: the groove's 2-point inset, the thumb 18 wide.
	local _, _, gw = box(app, 'role:group "REASONING"')
	local function at(f: number): number
		return 2 + math.floor(f * (gw - 4 - 18) + 0.5)
	end
	expect(app:state("test_picker_thumb")).to_be(at(0))
	-- From Low (the groove's start) to its middle, Medium: the thumb there
	-- the frame it's let go (it sprang after the level, a frame on still
	-- most of the way back).
	app:drag('role:group "REASONING"', 'role:group "REASONING"')
	app:frames(1)
	expect(app:state("test_picker_thumb")).to_be(at(0.5))
	expect(app:state("test_picker_effort")).to_be("medium")
	settle(app)
	expect(app:state("test_picker_thumb")).to_be(at(0.5))
end)

-- A new chat's composer, mid-page, grows down as its draft grows: its chip
-- held while its model picker is open (and the picker hanging from it, still),
-- it goes to its place as the picker closes. (On ui/chat_fixture.luau.)
test("a new chat's composer holds still under its open model picker", function()
	local app = mount("chat_fixture.luau")
	app:resize(960, 760)
	app:command("go_home")
	settle(app)
	local function chip_y(): number
		local _, y = box(app, '#chat-composer >> role:button "Model"')
		return y
	end
	local before = chip_y()
	app:click('#chat-composer >> role:button "Model"')
	settle(app)
	expect(listed(app)).to_be(true)
	local _, panel_y = box(app, "role:dialog")
	app:set_state("chat.composer.text", "one\ntwo\nthree\nfour")
	settle(app)
	expect(chip_y()).to_be(before)
	local _, panel_now = box(app, "role:dialog")
	expect(panel_now).to_be(panel_y)
	-- Closed: to its place (lower, as it grew down).
	app:key("Escape")
	settle(app)
	expect(listed(app)).to_be(false)
	expect(chip_y() > before).to_be(true)
	app:set_state("chat.composer.text", "")
	settle(app)
end)

-- The Control room's layout (screens/control/page): a page of its own in
-- the window, its head (its name, the counts, the tabs), each tab's blocks
-- with their titles and live figures in their heads. On
-- ui/control_page_fixture.luau, and in the shell (ui/control_fixture.luau).
-- (The tabs themselves: tests/control_tabs.test.luau and
-- tests/control_room_tabs.test.luau.)

local function open(w: number?, h: number?): any
	local app = mount("control_page_fixture.luau")
	app:resize(w or 1440, h or 900)
	app:frames(30)
	return app
end

local function tab(app: any, id: string)
	app:set_state("control_tab", id)
	app:frames(30)
end

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

test("each block is one of its own, its title and live figures in its head", function()
	local app = open(1440, 900)
	tab(app, "map")
	expect(app:has("#control-map >> .orch-block-head")).to_be(true)
	tab(app, "notes")
	for _, id in { "qa", "progress", "scratchpad" } do
		expect(app:has(`#control-notes-{id} >> .orch-block-head`)).to_be(true)
	end
	expect(app:has('#control-notes-progress >> .orch-block-head >> ~"working"')).to_be(true)
	-- The Board's figures, over it on its tab.
	tab(app, "board")
	expect(app:has('#control-board-figures >> ~"2 blocked"')).to_be(true)
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
	tab(app, "notes")
	-- A block's head: its title (sm, 12) and its figures (xs, 11).
	local title = baseline(app, "#control-notes-progress >> .orch-block-title", 12)
	local figure = baseline(app, '#control-notes-progress >> .orch-block-figures >> ~"working"', 11)
	expect(math.abs(title - figure) < 1).to_be(true)
	-- The page's title (lg, 15) and what's going on (sm, 12).
	local head = baseline(app, '#control-head >> "Control room"', 15)
	local summary = baseline(app, "#control-summary", 12)
	-- (Three sizes apart, the nudge is past the rounding: it shows.)
	expect(math.abs(head - summary) < 0.5).to_be(true)
end)

test("the head counts what the page shows, no glyphs between", function()
	local app = open(1440, 900)
	tab(app, "notes")
	local head = app:tree("#control-summary")
	-- Working: Progress's figure.
	local working = string.match(app:tree("#control-notes-progress >> .orch-block-head"), '"(%d+) working"')
	expect(working ~= nil).to_be(true)
	expect(head).to_contain(`"{working} working"`)
	-- Needing you: Needs you's count and the Q&A's answers not read yet
	-- (its block's "new" figure), counted once (data.waiting_for_you).
	local new = tonumber(string.match(app:tree("#control-notes-qa"), '"(%d+) new"')) or 0
	expect(new > 0).to_be(true)
	-- (Needs you's rows, floating over Notes, its rest opened: the board's
	-- three Needs-you cards.)
	if app:has("#control-needs-float >> .orch-asks-more") then
		app:click("#control-needs-float >> .orch-asks-more")
		app:frames(20)
	end
	expect(app:count("#control-needs-float >> .orch-need")).to_be(3)
	expect(head).to_contain(`"{3 + new} need you"`)
	-- Orchestrators: each of yours not archived (the Brain dump isn't one).
	expect(head).to_contain('"3 orchestrators"')
	expect(head).never.to_contain("·")
end)

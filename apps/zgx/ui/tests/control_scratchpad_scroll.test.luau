-- The Scratchpad in the Control room's page: a wheel over it scrolls the
-- page unless the person is working in it (Notes focused, the board
-- pressed), and the page's kept place (ds/kept_scroll) holds.

local function open(): any
	local app = mount("control_page_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	return app
end

local function place(app: any): number
	return app:state("control_scroll") or 0
end

-- Scrolled so the Scratchpad is in view, the page with room left below.
local function to_scratchpad(app: any)
	app:scroll("#control-scroll", 700)
	app:frames(30)
end

-- A note far longer than the field (the Brain dump adding to it).
local function long_note(app: any)
	for _ = 1, 15 do
		app:command("fixture.scratchpad_remote")
		app:frames(2)
	end
	app:frames(20)
end

test("scroll: a wheel over the unfocused Scratchpad scrolls the Control room, and its place is kept", function()
	local app = open()
	long_note(app)
	to_scratchpad(app)
	local at = place(app)
	expect(at > 0).to_be(true)
	-- (As in use: something else on the page pressed first.)
	app:click("#control-widget-scratchpad >> .orch-scratchpad-status")
	app:frames(10)
	expect(place(app)).to_be(at)
	-- Over the notes, a note longer than the field: the page moves.
	app:scroll("#scratchpad-editor", 120)
	app:frames(30)
	expect(place(app)).to_be(at + 120)
	app:scroll("#scratchpad-editor", -120)
	app:frames(30)
	expect(place(app)).to_be(at)
	-- Over the drawing, never pressed: the page moves, the view doesn't.
	app:click('#scratchpad-tabs >> role:radio "Draw"')
	app:frames(20)
	expect(place(app)).to_be(at)
	local board = "#control-widget-scratchpad >> .orch-scratchpad-board"
	app:scroll(board, 80)
	app:frames(30)
	expect(place(app)).to_be(at + 80)
	-- The kept place holds as the widget swaps its tabs about.
	app:click('#scratchpad-tabs >> role:radio "Notes"')
	app:frames(30)
	expect(place(app)).to_be(at + 80)
	expect(app:error()).to_be_nil()
end)

test("scroll: working in the Scratchpad keeps the wheel there", function()
	local app = open()
	long_note(app)
	to_scratchpad(app)
	local at = place(app)
	-- Notes focused: they scroll themselves, the page stays.
	app:click("#scratchpad-editor")
	app:frames(10)
	app:scroll("#scratchpad-editor", 120)
	app:frames(30)
	expect(place(app)).to_be(at)
	-- The board pressed: it pans, the page stays.
	app:click('#scratchpad-tabs >> role:radio "Draw"')
	app:frames(20)
	local board = "#control-widget-scratchpad >> .orch-scratchpad-board"
	app:click(board)
	app:frames(10)
	app:scroll(board, 80)
	app:frames(30)
	expect(place(app)).to_be(at)
	expect(app:error()).to_be_nil()
end)

-- A reply's thinking in the work group (ds/activity `thought_row`): its
-- words themselves in the tree, rendered as markdown, with no glyph, no
-- branch and no "Thought process" line of their own. Past four lines they
-- fold, fading out under "Show all N lines"; opened, "Show fewer lines"
-- folds them back. A thought that fits shows no button. Being written, the
-- fold shows the latest lines, rising through the window.

-- The small prose line (zeron.prose.small.line) and the fold's height.
local LINE = 20
local FOLD = 4 * LINE
-- The button's height, and its room under the words (ds/activity).
local BUTTON = 20
local ROOM = BUTTON + 6

-- A node's box by a pattern on its tree line (the first, or the last, so
-- matched): x, y, w, h.
local function box(app: any, pattern: string, last_one: boolean?): (number, number, number, number)
	local found: string? = nil
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			found = line
			if not last_one then
				break
			end
		end
	end
	assert(found, `nothing like {pattern}`)
	local x, y, w, h = (found :: string):match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
	return tonumber(x) :: number, tonumber(y) :: number, tonumber(w) :: number, tonumber(h) :: number
end

-- The "Show all N lines" button's N, if the button is there.
local function show_all(app: any): number?
	local n = app:tree():match('button "Show all (%d+) lines"')
	return if n then tonumber(n) else nil
end

-- How tall the thought's clip stands: the button sits in its room under
-- the clip, so from the words' first line (the clip's top, unless they've
-- risen) to the button's bottom, less the room.
local function clip_height(app: any, first_words: string): number
	local _, top = box(app, `text "{first_words}`)
	local _, b_y, _, b_h = box(app, 'button "Show')
	return b_y + b_h - ROOM - top
end

local function settle(app: any)
	app:frames(30)
end

test("a thought shows its words as markdown, folded to four lines under Show all", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s6")
	settle(app)
	-- The group opens to its rows.
	app:click('role:button ~"Thought process"')
	settle(app)
	-- The words, rendered: the markers are gone, and no row of its own
	-- names it (the one button so named is the group's summary).
	expect(app:has('~"Reading the frame"')).to_be(true)
	expect(app:has('~"Layout runs twice a frame."')).to_be(true)
	expect(app:has('~"**"')).to_be(false)
	expect(app:count('role:button "Thought process"')).to_be(1)
	-- Folded to four lines, the button over their foot, counting them all.
	expect(clip_height(app, "Reading the frame")).to_be(FOLD)
	local n = show_all(app)
	expect(n ~= nil).to_be(true)
	expect((n :: number) > 4).to_be(true)
	expect(app:has('role:button "Show fewer lines"')).to_be(false)
	-- Show all: the whole thought, Show fewer in the room under it.
	app:click('role:button ~"Show all"')
	settle(app)
	expect(app:has('role:button "Show fewer lines"')).to_be(true)
	expect(show_all(app)).to_be(nil)
	expect(clip_height(app, "Reading the frame") > FOLD + LINE).to_be(true)
	-- Show fewer: folded again, the same button in the same place.
	app:click('role:button "Show fewer lines"')
	settle(app)
	expect(show_all(app)).to_be(n)
	expect(clip_height(app, "Reading the frame")).to_be(FOLD)
	expect(app:error()).to_be(nil)
end)

test("opened stays opened: the chat left and come back to", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s6")
	settle(app)
	app:click('role:button ~"Thought process"')
	settle(app)
	app:click('role:button ~"Show all"')
	settle(app)
	app:command("go_s1")
	settle(app)
	app:command("go_s6")
	settle(app)
	expect(app:has('role:button "Show fewer lines"')).to_be(true)
	expect(clip_height(app, "Reading the frame") > FOLD + LINE).to_be(true)
	expect(app:error()).to_be(nil)
end)

-- Being written or settled, a thought past four lines folds the same way:
-- its first lines, fading out at the foot, the button under them. (It used
-- to show a written thought's latest lines, faded at the top.)
test("a thought being written folds to its first lines, as a settled one does", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	settle(app)
	app:command("thinking_turn")
	settle(app)
	-- Short so far: all of it, and no button (nothing to show more of).
	expect(app:has('~"Weighing the parser first."')).to_be(true)
	expect(show_all(app)).to_be(nil)
	local _, was_top = box(app, 'text "Weighing the parser first."')
	-- Past the fold while still written: a window four lines tall, the
	-- first line where it was, the button in its room under it.
	app:command("thought_grows")
	settle(app)
	expect(show_all(app) ~= nil).to_be(true)
	expect(clip_height(app, "Weighing the parser first.")).to_be(FOLD)
	local _, first_y = box(app, 'text "Weighing the parser first."')
	expect(first_y).to_be(was_top)
	-- Settled: its group folds shut (as the turn ends), and opened again,
	-- the same fold.
	app:command("thought_settles")
	settle(app)
	app:click('role:button ~"Thought process"')
	settle(app)
	expect(show_all(app) ~= nil).to_be(true)
	expect(clip_height(app, "Weighing the parser first.")).to_be(FOLD)
	local _, head_y = box(app, 'text "Weighing the parser first."')
	expect(head_y).to_be(was_top)
	expect(app:error()).to_be(nil)
end)

test("a thought that fits shows no button", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	settle(app)
	app:command("thinking_turn")
	settle(app)
	app:command("thought_settles")
	settle(app)
	app:click('role:button ~"Thought process"')
	settle(app)
	expect(app:has('~"Weighing the parser first."')).to_be(true)
	expect(show_all(app)).to_be(nil)
	expect(app:has('role:button "Show fewer lines"')).to_be(false)
	expect(app:count('role:button "Thought process"')).to_be(1)
	expect(app:error()).to_be(nil)
end)

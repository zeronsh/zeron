-- ⌘ held alone: the sidebar's first nine sessions, as ⌘1–9 go (what the
-- list shows: a group's sessions past "Show N more" left out), wear their
-- keys in place of their corner, at once; let go, they're gone at once.
-- When they show (zgx's `ui.modifiers`, a key pressed meanwhile, the
-- palette, the window) is src/shortcut_tests.rs's `jump_hints_*`: a Luau
-- test can't hold a modifier alone, so here the sidebar's own state is
-- set as the hold sets it. On the shell in ui/shell_fixture.luau.

local SIDE = "#shell-sidebar"

-- Whether keys are written macOS's way here: the sidebar's own New chat
-- says (⌘N, or Ctrl+N).
local mac = false

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	mac = app:has(`{SIDE} >> "⌘N"`)
	expect(mac or app:has(`{SIDE} >> "Ctrl+N"`)).to_be(true)
	return app
end

-- A jump's keys as this platform writes them: ⌘3 on macOS, Ctrl+3
-- elsewhere.
local function keys(n: number): string
	return if mac then `⌘{n}` else `Ctrl+{n}`
end

-- The jump a row wears (keys(3)), or "" for none.
local function wears(app: any, title: string): string
	local row = app:tree(`{SIDE} >> role:button "{title}"`)
	for n = 1, 9 do
		if row:find(`text "{keys(n)}" `, 1, true) then
			return keys(n)
		end
	end
	return ""
end

local function hold(app: any, on: boolean)
	app:set_state("shell_jump_hints", on)
end

test("⌘ held alone shows the first nine rows' jumps, skipping those past Show N more", function()
	local app = open()
	app:command("crowd_atlas")
	app:frames(30)
	expect(wears(app, "Fix the build")).to_be("")

	-- At once: no beat before them.
	hold(app, true)
	app:frames(2)
	expect(app:has(".jump-hint")).to_be(true)
	-- Atlas's five, then Borealis's two; Atlas 3–6 are folded away.
	local want = {
		["Fix the build"] = keys(1),
		["Write the docs"] = keys(2),
		["Tidy the tests"] = keys(3),
		["Atlas 1"] = keys(4),
		["Atlas 2"] = keys(5),
		["Ship the release"] = keys(6),
		["Answer the review"] = keys(7),
	}
	for title, keys in want do
		expect(`{title} {wears(app, title)}`).to_be(`{title} {keys}`)
	end
	expect(app:has(`{SIDE} >> role:button "Atlas 3"`)).to_be(false)

	-- Shown, Atlas's rest take their places; Borealis's go past nine.
	app:click(`{SIDE} >> role:button "Show 4 more"`)
	app:frames(40)
	expect(wears(app, "Atlas 3")).to_be(keys(6))
	expect(wears(app, "Atlas 6")).to_be(keys(9))
	expect(wears(app, "Ship the release")).to_be("")
	expect(wears(app, "Answer the review")).to_be("")

	-- Let go: gone at once, the corners back.
	hold(app, false)
	app:frames(2)
	expect(wears(app, "Fix the build")).to_be("")
	expect(app:has(".jump-hint")).to_be(false)
end)

-- The box of the first node `sel` names, from the tree's "x,y wxh".
local function box(app: any, sel: string): (number, number, number, number)
	local line = app:tree(sel):match("[^\n]+") or ""
	local x, y, w, h = line:match(" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
	assert(x, `no box for {sel}:\n{app:tree(sel)}`)
	return tonumber(x) :: number, tonumber(y) :: number, tonumber(w) :: number, tonumber(h) :: number
end

-- A jump's keys take the corner's place, as wide as they need: Ctrl+1 is
-- wider than ⌘1 or a time, and the subagents pill before it gives way,
-- never drawn over; within the row, at the sidebar's width and narrowed.
test("a jump's keys take the corner's width, the pill beside it never under them", function()
	local app = open()
	app:command("subagents_3")
	app:frames(30)
	local row = `{SIDE} >> role:button "Fix the build"`
	local pill = `{row} >> "3 subagents running"`
	expect(app:has(pill)).to_be(true)
	for _, width in { 0, 242 } do
		if width > 0 then
			app:set_state("shell_sidebar_width", width)
			app:frames(40)
		end
		hold(app, true)
		app:frames(4)
		expect(wears(app, "Fix the build")).to_be(keys(1))
		local hx, _, hw = box(app, `{row} >> "{keys(1)}"`)
		local px, _, pw = box(app, pill)
		local rx, _, rw = box(app, row)
		-- The pill ends before the keys start; the keys end inside the row.
		expect(`{width}: pill ends {px + pw}, keys start {hx}`).to_be(`{width}: pill ends {px + pw}, keys start {math.max(hx, px + pw)}`)
		expect(hx + hw <= rx + rw).to_be(true)
		hold(app, false)
		app:frames(4)
	end
end)

-- ⌘ held alone: the sidebar's first nine sessions, as ⌘1–9 go (what the
-- list shows: a group's sessions past "Show N more" left out), wear their
-- keys in place of their corner, at once; let go, they're gone at once.
-- When they show (zgx's `ui.modifiers`, a key pressed meanwhile, the
-- palette, the window) is src/shortcut_tests.rs's `jump_hints_*`: a Luau
-- test can't hold a modifier alone, so here the sidebar's own state is
-- set as the hold sets it. On the shell in ui/shell_fixture.luau.

local SIDE = "#shell-sidebar"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	return app
end

-- The jump a row wears ("⌘3"), or "" for none.
local function wears(app: any, title: string): string
	local row = app:tree(`{SIDE} >> role:button "{title}"`)
	return row:match("⌘%d") or ""
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
		["Fix the build"] = "⌘1",
		["Write the docs"] = "⌘2",
		["Tidy the tests"] = "⌘3",
		["Atlas 1"] = "⌘4",
		["Atlas 2"] = "⌘5",
		["Ship the release"] = "⌘6",
		["Answer the review"] = "⌘7",
	}
	for title, keys in want do
		expect(`{title} {wears(app, title)}`).to_be(`{title} {keys}`)
	end
	expect(app:has(`{SIDE} >> role:button "Atlas 3"`)).to_be(false)

	-- Shown, Atlas's rest take their places; Borealis's go past nine.
	app:click(`{SIDE} >> role:button "Show 4 more"`)
	app:frames(40)
	expect(wears(app, "Atlas 3")).to_be("⌘6")
	expect(wears(app, "Atlas 6")).to_be("⌘9")
	expect(wears(app, "Ship the release")).to_be("")
	expect(wears(app, "Answer the review")).to_be("")

	-- Let go: gone at once, the corners back.
	hold(app, false)
	app:frames(2)
	expect(wears(app, "Fix the build")).to_be("")
	expect(app:has(".jump-hint")).to_be(false)
end)

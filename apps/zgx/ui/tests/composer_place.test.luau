-- The composer's footer over a new chat's wallpaper (ui/composer_place_
-- fixture.luau): the place chip ([tile] project › branch), the faint
-- backing its chips take only over art, which part of the chip gives way
-- first as room runs short, and the card the context ring opens.

local function settle(app: any)
	app:frames(30)
end

-- Settled, and a label's roll (RollText, about 750 ms) run out.
local function rested(app: any)
	app:wait("#never-shown", { timeout_ms = 900 })
	settle(app)
end

local function mounted(): any
	local app = mount("composer_place_fixture.luau")
	app:resize(900, 360)
	settle(app)
	return app
end

-- Whether a node is on screen (the tree marks one that isn't).
local function showing(app: any, sel: string): boolean
	if not app:has(sel) then
		return false
	end
	local first = string.match(app:tree(sel .. "[1]"), "[^\n]+") or ""
	return string.find(first, " hidden", 1, true) == nil
end

-- A part's laid-out width: the clip around its text (the first box the
-- selector prints).
local function width(app: any, sel: string): number
	local first = string.match(app:tree(sel .. "[1]"), "[^\n]+") or ""
	return tonumber(string.match(first, " %-?%d+,%-?%d+ (%d+)x%d+")) or -1
end

test("composer place: over a wallpaper the footer sits on a tray, bare without", function()
	local app = mounted()
	-- Over art: the pill's glass continued under the row, one tray; no
	-- backing of any chip's own.
	expect(app:count(".composer-tray")).to_be(1)
	expect(app:count(".composer-over-art")).to_be(0)
	-- No art: no tray (gone once it's faded).
	app:set_state("test_place_art", false)
	rested(app)
	expect(app:count(".composer-tray")).to_be(0)
	-- And back.
	app:set_state("test_place_art", true)
	rested(app)
	expect(app:count(".composer-tray")).to_be(1)
	expect(app:error()).to_be(nil)
end)

test("composer place: where it runs leads, a worktree's glyph or a folder, then project › branch", function()
	local app = mounted()
	rested(app)
	-- A worktree: its glyph, the project's name › the branch. No tile.
	expect(app:has("#composer-place-lead >> .place-lead-worktree")).to_be(true)
	expect(app:text("#composer-place-lead")).to_be("")
	expect(app:text("#composer-place-project[1]")).to_be("zeron")
	expect(showing(app, "#composer-place-branch")).to_be(true)
	expect(app:text("#composer-place-branch[1]")).to_be("zeron/create-readme-update-for-onboarding")
	-- The project's own checkout: a folder.
	app:set_state("test_place_checkout", "local")
	rested(app)
	expect(app:has("#composer-place-lead >> .place-lead-folder")).to_be(true)
	-- No project: the folder, "No project", no branch.
	app:set_state("test_place_checkout", "worktree")
	app:set_state("test_place_project", "none")
	rested(app)
	expect(app:has("#composer-place-lead >> .place-lead-folder")).to_be(true)
	expect(app:text("#composer-place-project[1]")).to_be("No project")
	expect(showing(app, "#composer-place-branch")).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("composer place: short of room the branch gives way first, then the project keeps its least", function()
	local app = mounted()
	app:set_state("test_place_project", "atlas")
	app:set_state("test_place_w", 840)
	rested(app)
	-- Roomy: the project's name at its most, the branch at its most.
	local project_wide = width(app, "#composer-place-project")
	local branch_wide = width(app, "#composer-place-branch")
	expect(project_wide).to_be(140)
	expect(branch_wide).to_be(180)
	-- Narrower: the branch gives way, the project's name doesn't (the
	-- access mode folds to its icon to keep some of the branch).
	app:set_state("test_place_w", 440)
	rested(app)
	expect(width(app, "#composer-place-project")).to_be(project_wide)
	local b = width(app, "#composer-place-branch")
	expect(b < branch_wide and b > 36).to_be(true)
	-- Narrower still: the branch down to its sliver, and only then the
	-- project's name, to no less than 44 of it.
	app:set_state("test_place_w", 360)
	rested(app)
	expect(width(app, "#composer-place-branch")).to_be(36)
	local p = width(app, "#composer-place-project")
	expect(p < project_wide and p >= 44).to_be(true)
	-- Past that: the glyph alone.
	app:set_state("test_place_w", 270)
	rested(app)
	expect(showing(app, "#composer-place-project")).to_be(false)
	expect(app:has("#composer-place-lead >> .place-lead-worktree")).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("composer place: the context ring opens a card of the numbers, as Zeron writes them", function()
	local app = mounted()
	app:hover("#composer-context")
	rested(app)
	expect(app:has("#composer-context-card")).to_be(true)
	expect(app:text("#composer-context-used")).to_be("408,389 / 1,000,000 tokens")
	expect(app:text("#composer-context-left")).to_be("591,611 tokens remaining")
	expect(app:has('#composer-context-card >> "41%"')).to_be(true)
	-- Past the window: nothing left, not less than nothing.
	app:set_state("test_place_usage", { tokens = 1048576, window = 1000000 })
	settle(app)
	expect(app:text("#composer-context-left")).to_be("0 tokens remaining")
	expect(app:has('#composer-context-card >> "100%"')).to_be(true)
	-- A window with nothing counted yet.
	app:set_state("test_place_usage", { window = 200000 })
	settle(app)
	expect(app:text("#composer-context-used")).to_be("200,000 token capacity")
	expect(app:text("#composer-context-left")).to_be("Waiting for context usage")
	expect(app:error()).to_be(nil)
end)

test("composer place: the context ring shows only once a window is reported", function()
	local app = mounted()
	expect(showing(app, "#composer-context")).to_be(true)
	app:set_state("test_place_usage", nil)
	settle(app)
	expect(showing(app, "#composer-context")).to_be(false)
	-- Tokens alone, no window: still none (an agent that never says one).
	app:set_state("test_place_usage", { tokens = 5417 })
	settle(app)
	expect(showing(app, "#composer-context")).to_be(false)
	app:set_state("test_place_usage", { tokens = 5417, window = 1048576 })
	settle(app)
	expect(showing(app, "#composer-context")).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("composer place: the footer's chips are one height, the ring one of them", function()
	local app = mounted()
	rested(app)
	local function box(sel: string): (number, number)
		local first = string.match(app:tree(sel), "[^\n]+") or ""
		local w, h = string.match(first, " %-?%d+,%-?%d+ (%d+)x(%d+)")
		return tonumber(w) or -1, tonumber(h) or -1
	end
	local _, access = box('role:button "Access"')
	local _, place = box('role:button "Branch"')
	local _, model = box('role:button "Model"')
	local _, ring = box("#composer-context")
	expect(access).to_be(24)
	expect(place).to_be(24)
	expect(model).to_be(24)
	expect(ring).to_be(24)
	-- The ring at the chips' glyph size (12), 6 in all round: a square.
	local w = box("#composer-context")
	expect(w).to_be(24)
end)

-- A hover card is framed once (ds/hover_card): zgx's layer hugs the card
-- exactly (its box the card's), and one frame paints in it. A card that
-- painted its own shadow, or a second frame round it, shows as a dark rim
-- round a bigger box in the popup's window.
test("composer place: the context card is framed once, its layer hugging it", function()
	local app = mounted()
	app:hover("#composer-context")
	rested(app)
	local function box(sel: string): { number }
		local first = string.match(app:tree(sel), "[^\n]+") or ""
		local x, y, w, h = string.match(first, " (%-?%d+),(%-?%d+) (%d+)x(%d+)")
		return { tonumber(x) or -1, tonumber(y) or -1, tonumber(w) or -1, tonumber(h) or -1 }
	end
	local layer = box('role:dialog "Tooltip"')
	local card = box("#composer-context-card")
	for i = 1, 4 do
		expect(math.abs(layer[i] - card[i]) <= 0.5).to_be(true)
	end
	expect(app:count('role:dialog "Tooltip" >> .hover-card-frame')).to_be(1)
	expect(app:error()).to_be(nil)
end)

-- On the tray the row keeps the pill's spacing: its chips sit as far below
-- the pill's edge as + sits above it, and line up with + and Send.
test("composer place: on the tray the chips keep the pill's inset, lined up with + and Send", function()
	local app = mounted()
	rested(app)
	local function box(sel: string): { number }
		local first = string.match(app:tree(sel), "[^\n]+") or ""
		local x, y, w, h = string.match(first, " (%-?%d+),(%-?%d+) (%d+)x(%d+)")
		return { tonumber(x) or -1, tonumber(y) or -1, tonumber(w) or -1, tonumber(h) or -1 }
	end
	local plus, send = box('role:button "Attach"'), box('role:button "Send message"')
	local access, ring = box('role:button "Access"'), box("#composer-context")
	-- + sits 12 above the pill's edge (11 inside its hairline); the chips
	-- A tight step (6) below the pill: 18 from +'s foot to the chips' top.
	expect(access[2] - (plus[2] + plus[4])).to_be(18)
	-- Their outer edges on +'s and Send's.
	expect(access[1]).to_be(plus[1])
	expect(ring[1] + ring[3]).to_be(send[1] + send[3])
	-- Bare: the footer as ever (8 under the pill, 10 in).
	app:set_state("test_place_art", false)
	rested(app)
	access, plus = box('role:button "Access"'), box('role:button "Attach"')
	expect(access[2] - (plus[2] + plus[4])).to_be(20)
	expect(app:error()).to_be(nil)
end)

-- zgx's own menus (the built-in text menu) wear our menus' panel tint and
-- row wash (ds/tokens), in both themes, not zgx's highlight.
test("composer place: zgx's built-in menus take our menu tint and row wash", function()
	local app = mounted()
	for _, dark in { true, false } do
		app:set_state("test_place_dark", dark)
		app:frames(5)
		local t = app:state("test_place_menu_tokens") :: any
		expect(t.menu_bg).to_be(t.tint)
		expect(t.menu_hover).to_be(t.hover)
	end
	expect(app:error()).to_be(nil)
end)

-- What a chip says, a line each for what it shows: its text, nothing when folded
-- to its icon.
local function chip_text(app: any, name: string): string
	local out = {}
	for line in app:tree(`role:button "{name}"`):gmatch("[^\n]+") do
		if not line:find(" hidden", 1, true) then
			local text = line:match('text "([^"]+)"') or line:match('#[%w%-]+ "([^"]+)"')
			if text then
				table.insert(out, text)
			end
		end
	end
	return table.concat(out, " ")
end

test("composer footer: short of room the effort goes first, the model's name last", function()
	local app = mount("chat_fixture.luau")
	app:resize(800, 500)
	app:frames(60)
	expect(chip_text(app, "Model")).to_contain("High")
	-- Narrowing: the effort is gone while the access mode still has its name.
	local effort_gone, access_folded = nil :: number?, nil :: number?
	for w = 600, 360, -4 do
		app:resize(w, 500)
		app:frames(10)
		if not effort_gone and not chip_text(app, "Model"):find("High", 1, true) then
			effort_gone = w
		end
		if not access_folded and chip_text(app, "Access") == "" then
			access_folded = w
		end
	end
	expect(effort_gone ~= nil and access_folded ~= nil).to_be(true)
	expect((effort_gone :: number) > (access_folded :: number)).to_be(true)
	expect(chip_text(app, "Model")).to_contain("Opus")
	-- Wide again, all of it.
	app:resize(800, 500)
	app:frames(60)
	expect(chip_text(app, "Model")).to_contain("High")
	expect(chip_text(app, "Access")).never.to_be("")
	expect(app:error()).to_be_nil()
end)

test("composer footer: the model's name goes only after everything else has folded", function()
	local app = mounted()
	app:set_state("test_place_w", 800)
	rested(app)
	expect(chip_text(app, "Model")).to_contain("Opus")
	app:set_state("test_place_w", 260)
	rested(app)
	expect(chip_text(app, "Access")).to_be("")
	expect(chip_text(app, "Branch")).to_be("")
	expect(chip_text(app, "Model")).to_contain("Opus")
	app:set_state("test_place_w", 180)
	rested(app)
	expect(chip_text(app, "Model")).to_be("")
	expect(width(app, 'role:button "Model"')).to_be(24)
	-- Back with room, its name again.
	app:set_state("test_place_w", 800)
	rested(app)
	expect(chip_text(app, "Model")).to_contain("Opus")
	expect(app:error()).to_be_nil()
end)

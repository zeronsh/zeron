-- A split's panes and the two sides: opening one side when the panes wouldn't
-- have their least with both open hides the other, three columns at most
-- (Wing's cap); hidden by the fit, not closed, it's back by itself once
-- there's room again (the side opened closed). What the person closed stays
-- closed. A window too narrow: the panel's width goes first, then the
-- sidebar hides, then the panes narrow. The panel never grows by itself
-- beside them, and nothing covers them. On the shell in
-- ui/shell_fixture.luau.

local PANE_MIN = require("../screens/shell/fit_policy").PANE_MIN

local function box(app: any, target: string): (number, number)
	local line = app:tree(target):match("[^\n]+") or ""
	local x, w = line:match(" (%-?%d+),%-?%d+ (%d+)x%d+")
	return tonumber(x) or 0, tonumber(w) or 0
end
local function pane_w(app: any, id: string): number
	local _, w = box(app, `#split-pane-{id}`)
	return w
end
local function panel_w(app: any): number
	local _, w = box(app, "#shell-files")
	return w
end
local function sidebar_shows(app: any): boolean
	local _, w = box(app, "#shell-sidebar")
	return w > 1
end
local function expanded(app: any, id: string): boolean
	return (app:state("shell_panel_expanded") :: any)[id] == true
end
local function leaf(id: string): any
	return { kind = "leaf", id = id }
end
local function row(ids: { string }): any
	local kids = {}
	for _, id in ids do
		table.insert(kids, leaf(id))
	end
	return { kind = "row", children = kids }
end
local function toggle(app: any)
	app:click('#titlebar >> role:button "Panel"')
	app:frames(60)
end
local function toggle_sidebar(app: any)
	app:click('#titlebar >> role:button "Toggle sidebar"')
	app:frames(60)
end
-- The shell at `w` wide on Fix the build (a1), in `tree`.
local function split(w: number, tree: any?): any
	local app = mount("shell_fixture.luau")
	app:resize(w, 800)
	app:frames(30)
	if tree then
		app:set_state("inset_split_trees", { tree })
	end
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(30)
	return app
end
local function expect_panes_keep_their_least(app: any, ids: { string }, when: string)
	for _, id in ids do
		local w = pane_w(app, id)
		expect(`{when}, {id} {w}: {w >= PANE_MIN}`).to_be(`{when}, {id} {w}: true`)
	end
end
local PAIR, TRIO = { "a1", "a2" }, { "a1", "a2", "a3" }

test("the panel opened when the panes would go under their least: the sidebar gives way, and is back as it closes", function()
	local app = split(1100, row(PAIR))
	expect(sidebar_shows(app)).to_be(true)
	toggle(app)
	expect(panel_w(app)).to_be(360)
	expect_panes_keep_their_least(app, PAIR, "1100")
	expect(sidebar_shows(app)).to_be(false)
	-- Hidden for room, not closed by the person.
	expect(app:state("shell_collapsed")).to_be(false)
	-- The panel closed: the sidebar is back by itself.
	toggle(app)
	expect(panel_w(app) <= 1).to_be(true)
	expect(sidebar_shows(app)).to_be(true)
	-- The sidebar folded by the person, the panel opened and closed: it
	-- stays folded.
	toggle_sidebar(app)
	expect(sidebar_shows(app)).to_be(false)
	toggle(app)
	toggle(app)
	expect(sidebar_shows(app)).to_be(false)
	expect(app:state("shell_collapsed")).to_be(true)
end)

test("the sidebar opened when the panes would go under their least: the panel gives way, and is back as it folds", function()
	local app = split(1100, row(PAIR))
	toggle(app)
	expect(sidebar_shows(app)).to_be(false)
	toggle_sidebar(app)
	expect(sidebar_shows(app)).to_be(true)
	expect(panel_w(app) <= 1).to_be(true)
	-- The sidebar folded: the panel is back by itself, at its least.
	toggle_sidebar(app)
	expect(sidebar_shows(app)).to_be(false)
	expect(panel_w(app)).to_be(360)
	-- The panel closed by the person, the sidebar opened and folded: it
	-- stays closed.
	toggle(app)
	toggle_sidebar(app)
	toggle_sidebar(app)
	expect(panel_w(app) <= 1).to_be(true)
end)

test("with room for both, panes at their least, nothing closes either way", function()
	local app = split(1440, row(PAIR))
	toggle(app)
	expect(sidebar_shows(app)).to_be(true)
	expect(panel_w(app) >= 360).to_be(true)
	expect_panes_keep_their_least(app, PAIR, "1440")
	toggle_sidebar(app)
	expect(sidebar_shows(app)).to_be(false)
	toggle_sidebar(app)
	expect(sidebar_shows(app)).to_be(true)
	expect(panel_w(app) >= 360).to_be(true)
end)

test("under their least with only one side open, the panes narrow: the panel is a column beside them at its least", function()
	local app = split(1100, row(TRIO))
	toggle(app)
	expect(sidebar_shows(app)).to_be(false)
	expect(panel_w(app)).to_be(360)
	expect(expanded(app, "a1")).to_be(false)
	-- 1100 less the panel, the window's gaps and those between: shared.
	for _, id in TRIO do
		local w = pane_w(app, id)
		expect(`{id} {w}: {w >= 239 and w <= 241}`).to_be(`{id} {w}: true`)
	end
	-- And closes as it opened.
	toggle(app)
	expect(panel_w(app) <= 1).to_be(true)
	expect(pane_w(app, "a1") > 300).to_be(true)
end)

test("the sidebar opened beside panes under their least with the panel closed: they narrow", function()
	local app = split(1100, row(TRIO))
	toggle(app)
	toggle(app)
	toggle_sidebar(app)
	expect(sidebar_shows(app)).to_be(true)
	expect(panel_w(app) <= 1).to_be(true)
	expect(pane_w(app, "a1") < PANE_MIN).to_be(true)
end)

-- (The panel's share: half of what the sidebar and the window's gaps round
-- the cards leave, 18 beside the sidebar, 14 without: chat and panel equal.)
test("a chat alone goes as it always has: the sidebar stays, or hides for room and comes back", function()
	local app = split(1100)
	toggle(app)
	expect(sidebar_shows(app)).to_be(true)
	expect(panel_w(app)).to_be(405)
	toggle(app)
	app:resize(880, 800)
	app:frames(30)
	toggle(app)
	expect(panel_w(app)).to_be(433)
	expect(sidebar_shows(app)).to_be(false)
	expect(app:state("shell_collapsed")).to_be(false)
	toggle(app)
	expect(sidebar_shows(app)).to_be(true)
end)

test("a chat alone in a narrow window: anything opening its panel shows it, as its toggle does", function()
	-- (Files' key, as a file link or a browser link opens it: under 902 the
	-- panel came up hidden behind the sidebar, open and nowhere to be seen.)
	local app = split(880)
	expect(sidebar_shows(app)).to_be(true)
	app:command("shell.toggle_files_tab")
	app:frames(60)
	expect(panel_w(app)).to_be(433)
	expect(sidebar_shows(app)).to_be(false)
	-- Closed by its toggle, the sidebar's back, as ever.
	toggle(app)
	expect(panel_w(app) <= 1).to_be(true)
	expect(sidebar_shows(app)).to_be(true)
	-- With room for both, the sidebar stays.
	app:resize(1000, 800)
	app:frames(30)
	app:command("shell.toggle_files_tab")
	app:frames(60)
	expect(panel_w(app)).to_be(360)
	expect(sidebar_shows(app)).to_be(true)
end)

test("panes stacked are as wide as the page: one across, as a chat alone", function()
	local app = split(1100, { kind = "col", children = { leaf("a1"), leaf("a2") } })
	toggle(app)
	expect(sidebar_shows(app)).to_be(true)
	expect(panel_w(app)).to_be(405)
end)

test("the window narrowed: the panel gives its width, then the sidebar hides, then the panes narrow; widened, the sidebar's back", function()
	local app = split(1500, row(PAIR))
	toggle(app)
	app:click('role:slider "Resize files"')
	app:frames(5)
	for _ = 1, 8 do
		app:key("shift+ArrowLeft")
		app:frames(2)
	end
	app:frames(40)
	local wide = panel_w(app)
	expect(wide > 360).to_be(true)
	local kept = pane_w(app, "a1")
	local w, hid_at = 1500, nil :: number?
	while w > 760 do
		w -= 20
		app:resize(w, 800)
		app:frames(20)
		expect(`{w}: {panel_w(app) >= 360}`).to_be(`{w}: true`)
		if panel_w(app) > 360 then
			-- Its width first: the panes keep theirs, or gain the sidebar's.
			expect(`{w}: {pane_w(app, "a1") >= kept - 1}`).to_be(`{w}: true`)
		end
		if not sidebar_shows(app) and not hid_at then
			hid_at = w
		end
		-- The sidebar before the panes go narrower than the panel.
		if sidebar_shows(app) and panel_w(app) <= 360 then
			expect(`{w}: {pane_w(app, "a1") >= panel_w(app) - 8}`).to_be(`{w}: true`)
		end
	end
	expect(hid_at ~= nil).to_be(true)
	expect(pane_w(app, "a1") < PANE_MIN).to_be(true)
	-- Hidden for room, not closed: the room back, the sidebar back.
	expect(app:state("shell_collapsed")).to_be(false)
	app:resize(1500, 800)
	app:frames(60)
	expect(sidebar_shows(app)).to_be(true)
end)

test("the window widened beside a split: the panes take it all, the panel keeps its width", function()
	local app = split(1100, row(PAIR))
	toggle(app)
	expect(panel_w(app)).to_be(360)
	local was = pane_w(app, "a1")
	app:resize(1800, 800)
	app:frames(60)
	expect(panel_w(app)).to_be(360)
	expect(pane_w(app, "a1") > was).to_be(true)
end)

test("beside a split it opens at its least, whatever width was chosen before", function()
	local app = split(1800, leaf("a1"))
	toggle(app)
	app:click('role:slider "Resize files"')
	app:frames(5)
	for _ = 1, 20 do
		app:key("shift+ArrowLeft")
		app:frames(2)
	end
	app:frames(40)
	local alone = panel_w(app)
	expect(alone > 360).to_be(true)
	toggle(app)
	app:set_state("inset_split_trees", { row(PAIR) })
	app:frames(40)
	toggle(app)
	expect(panel_w(app)).to_be(360)
end)

test("the sidebar hidden for room: its toggle shows it, the panel closing, and it's the panes that narrow", function()
	local app = split(1000, row(TRIO))
	expect(sidebar_shows(app)).to_be(false)
	expect(app:state("shell_collapsed")).to_be(false)
	toggle_sidebar(app)
	expect(sidebar_shows(app)).to_be(true)
	expect(app:state("shell_collapsed")).to_be(false)
	-- Asked for here it stays; a narrower window, and it gives way again.
	app:resize(1000, 800)
	app:frames(30)
	expect(sidebar_shows(app)).to_be(true)
	app:resize(900, 800)
	app:frames(60)
	expect(sidebar_shows(app)).to_be(false)
end)

test("a pane added beside the panel: its width goes first, then the sidebar; a pane closed, the panes have the room", function()
	local app = split(1500, row(PAIR))
	toggle(app)
	app:click('role:slider "Resize files"')
	app:frames(5)
	for _ = 1, 30 do
		app:key("shift+ArrowLeft")
		app:frames(2)
	end
	app:frames(40)
	expect_panes_keep_their_least(app, PAIR, "two")
	app:set_state("inset_split_trees", { row({ "a1", "a2", "a3", "b1" }) })
	app:frames(60)
	expect(panel_w(app)).to_be(360)
	expect(sidebar_shows(app)).to_be(false)
	expect(expanded(app, "a1")).to_be(false)
	app:set_state("inset_split_trees", { row(PAIR) })
	app:frames(60)
	expect(panel_w(app)).to_be(360)
	expect(sidebar_shows(app)).to_be(true)
end)

test("its edge stops at the panes' least, and its width restored keeps to it", function()
	local app = split(1440, row(PAIR))
	toggle(app)
	app:click('role:slider "Resize files"')
	app:frames(5)
	for _ = 1, 30 do
		app:key("shift+ArrowLeft")
		app:frames(2)
	end
	app:frames(40)
	expect_panes_keep_their_least(app, PAIR, "dragged wide")
	local w = pane_w(app, "a1")
	expect(`{w} {w <= PANE_MIN + 1}`).to_be(`{w} true`)
	toggle(app)
	toggle(app)
	expect_panes_keep_their_least(app, PAIR, "restored")
	-- The window wider: it takes back the width dragged (its tree docked now
	-- too), then the panes grow.
	app:resize(1900, 800)
	app:frames(40)
	expect(pane_w(app, "a1") > w).to_be(true)
end)

test("beside a split its tree floats, however wide the window: the panel never grows for it", function()
	local app = split(1440, row(PAIR))
	toggle(app)
	app:click("#panel-open-files")
	app:frames(40)
	expect_panes_keep_their_least(app, PAIR, "floating")
	expect(panel_w(app)).to_be(360)
	app:resize(1800, 800)
	app:frames(40)
	expect(panel_w(app)).to_be(360)
	expect_panes_keep_their_least(app, PAIR, "wide")
end)

test("opened by anything else: Files' key hides the sidebar too, and it's back as the panel closes", function()
	local app = split(1100, row(PAIR))
	app:command("shell.toggle_files_tab")
	app:frames(60)
	expect(panel_w(app)).to_be(360)
	expect(sidebar_shows(app)).to_be(false)
	toggle(app)
	expect(sidebar_shows(app)).to_be(true)
end)

test("Expand is only ever the person's: the sidebar that gave way is back over it, and folding it closes nothing", function()
	local app = split(1100, row(TRIO))
	toggle(app)
	expect(sidebar_shows(app)).to_be(false)
	expect(expanded(app, "a1")).to_be(false)
	app:click("#panel-expand")
	app:frames(60)
	expect(expanded(app, "a1")).to_be(true)
	-- (Expanded over the panes, there's room for it again.)
	expect(sidebar_shows(app)).to_be(true)
	toggle_sidebar(app)
	expect(sidebar_shows(app)).to_be(false)
	expect(expanded(app, "a1")).to_be(true)
	expect(panel_w(app) > 500).to_be(true)
end)

test("a pane added as the sidebar steps aside: the panes and the composer keep together as they ease", function()
	local app = split(1440, row(PAIR))
	toggle(app)
	local function span(target: string): (number, number)
		local x, w = (app:tree(target):match("[^\n]+") :: string):match(" (%-?%d+),%-?%d+ (%d+)x%d+")
		return tonumber(x) :: number, tonumber(w) :: number
	end
	local focused = if (app:tree("#shell-page"):match(" (%-?%d+),") == app:tree("#split-pane-a1"):match(" (%-?%d+),")) then "a1" else "a2"
	app:set_state("inset_split_trees", { row(TRIO) })
	local last_a1: number? = nil
	for frame = 1, 40 do
		app:frames(1)
		local x1, w1 = span("#split-pane-a1")
		local x2, w2 = span("#split-pane-a2")
		local x3 = span("#split-pane-a3")
		-- Never into each other, whatever the page is doing: the new one too.
		expect(`frame {frame}: {x1 + w1 <= x2 + 1}`).to_be(`frame {frame}: true`)
		expect(`frame {frame}: {x2 + w2 <= x3 + 1}`).to_be(`frame {frame}: true`)
		-- The composer's frame goes with its pane, not ahead of it.
		local px = span("#shell-page")
		local fx = span(`#split-pane-{focused}`)
		expect(`frame {frame}: {math.abs(px - fx) <= 1}`).to_be(`frame {frame}: true`)
		-- No snap into place at the end: late on, only small steps.
		if frame > 20 and last_a1 then
			expect(`frame {frame}: {math.abs(w1 - last_a1) <= 12}`).to_be(`frame {frame}: true`)
		end
		last_a1 = w1
	end
	-- Settled, three alike across the room.
	local _, w1 = span("#split-pane-a1")
	local _, w3 = span("#split-pane-a3")
	expect(math.abs(w1 - w3) <= 1).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("a chat landing on a lone chat: that chat eases down from the whole page, the new one opening beside it", function()
	local app = split(1440)
	local page_x, page_w = box(app, "#shell-page")
	expect(page_w > 600).to_be(true)
	app:set_state("inset_split_trees", { row(PAIR) })
	local narrowest = math.huge
	local last_w: number? = nil
	for frame = 1, 40 do
		app:frames(1)
		local x1, w1 = box(app, "#split-pane-a1")
		local x2 = box(app, "#split-pane-a2")
		-- It keeps its place at the page's left: never out of its zero width.
		expect(`frame {frame}: {math.abs(x1 - page_x) <= 1}`).to_be(`frame {frame}: true`)
		-- Only ever narrowing, to its half: no collapsing first and growing back.
		if last_w then
			expect(`frame {frame}: {w1 <= last_w + 1}`).to_be(`frame {frame}: true`)
		end
		last_w = w1
		narrowest = math.min(narrowest, w1)
		expect(`frame {frame}: {x1 + w1 <= x2 + 1}`).to_be(`frame {frame}: true`)
	end
	local _, w1 = box(app, "#split-pane-a1")
	local _, w2 = box(app, "#split-pane-a2")
	expect(math.abs(w1 - w2) <= 1).to_be(true)
	expect(narrowest >= w1 - 1).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("a split with more panes shown: the sidebar steps aside for room, and isn't closed", function()
	local app = split(1440, row(PAIR))
	toggle(app)
	expect(sidebar_shows(app)).to_be(true)
	-- The panel's another chat's; a split of three shown there, the panel open.
	app:set_state("inset_split_trees", { row(TRIO) })
	app:frames(60)
	expect(sidebar_shows(app)).to_be(false)
	expect(app:state("shell_collapsed")).to_be(false)
	expect(panel_w(app)).to_be(360)
	expect(app:has("#split-pane-a3")).to_be(true)
end)

test("a pane's footer with no project keeps Full access while it fits", function()
	-- (`No ref` alone takes little: the footer doesn't save room for a name it
	-- doesn't show, and leave a gap where the access mode's words would go.)
	local app = split(1150, row({ "a1", "a2" }))
	expect(app:tree('#split-pane-a2 >> role:button "Access"')).to_contain("Full access")
	local _, access = box(app, '#split-pane-a2 >> role:button "Access"')
	expect(access > 24).to_be(true)
end)

-- A split's panes and the two sides: opening one side when the panes wouldn't
-- have their least with both open closes the other, which stays closed until
-- it's asked for. With one side open and still too little, the panes narrow;
-- nothing covers them and nothing hides by itself. On the shell in
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

test("the panel opened when the panes would go under their least: the sidebar closes, and stays closed", function()
	local app = split(1100, row(PAIR))
	expect(sidebar_shows(app)).to_be(true)
	toggle(app)
	expect(panel_w(app)).to_be(360)
	expect_panes_keep_their_least(app, PAIR, "1100")
	expect(sidebar_shows(app)).to_be(false)
	expect(app:state("shell_collapsed")).to_be(true)
	-- The panel closed: the sidebar isn't back until it's asked for.
	toggle(app)
	expect(panel_w(app) <= 1).to_be(true)
	expect(sidebar_shows(app)).to_be(false)
	toggle_sidebar(app)
	expect(sidebar_shows(app)).to_be(true)
end)

test("the sidebar opened when the panes would go under their least: the panel closes, and stays closed", function()
	local app = split(1100, row(PAIR))
	toggle(app)
	expect(sidebar_shows(app)).to_be(false)
	toggle_sidebar(app)
	expect(sidebar_shows(app)).to_be(true)
	expect(panel_w(app) <= 1).to_be(true)
	-- The sidebar closed: the panel isn't back until it's asked for.
	toggle_sidebar(app)
	expect(sidebar_shows(app)).to_be(false)
	expect(panel_w(app) <= 1).to_be(true)
	-- Asked for, it's back, and the sidebar's closed already: nothing else.
	toggle(app)
	expect(panel_w(app)).to_be(360)
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

test("a chat alone goes as it always has: the sidebar stays, or hides for room and comes back", function()
	local app = split(1100)
	toggle(app)
	expect(sidebar_shows(app)).to_be(true)
	expect(panel_w(app)).to_be(360)
	toggle(app)
	app:resize(880, 800)
	app:frames(30)
	toggle(app)
	expect(panel_w(app)).to_be(360)
	expect(sidebar_shows(app)).to_be(false)
	expect(app:state("shell_collapsed")).to_be(false)
	toggle(app)
	expect(sidebar_shows(app)).to_be(true)
end)

test("panes stacked are as wide as the page: one across, as a chat alone", function()
	local app = split(1100, { kind = "col", children = { leaf("a1"), leaf("a2") } })
	toggle(app)
	expect(sidebar_shows(app)).to_be(true)
	expect(panel_w(app)).to_be(360)
end)

test("the window narrowed step by step: nothing closes, the panel gives its width then the panes narrow", function()
	local app = split(1500, row(PAIR))
	toggle(app)
	expect(sidebar_shows(app)).to_be(true)
	local w = 1500
	while w > 950 do
		w -= 25
		app:resize(w, 800)
		app:frames(20)
		expect(`{w}: {sidebar_shows(app)}`).to_be(`{w}: true`)
		expect(`{w}: {panel_w(app) >= 360}`).to_be(`{w}: true`)
		-- (Its width first, the panes keeping their least while it has any.)
		if panel_w(app) > 360 then
			expect_panes_keep_their_least(app, PAIR, `{w}`)
		end
	end
	expect(panel_w(app)).to_be(360)
	expect(pane_w(app, "a1") < PANE_MIN).to_be(true)
	expect(app:state("shell_collapsed")).to_be(false)
end)

test("a pane added beside the panel: nothing hides, the panes narrow; closed, the panel has its room back", function()
	local app = split(1500, row(PAIR))
	toggle(app)
	app:click('role:slider "Resize files"')
	app:frames(5)
	for _ = 1, 30 do
		app:key("shift+ArrowLeft")
		app:frames(2)
	end
	app:frames(40)
	local two = panel_w(app)
	expect_panes_keep_their_least(app, PAIR, "two")
	app:set_state("inset_split_trees", { row({ "a1", "a2", "a3", "b1" }) })
	app:frames(60)
	expect(panel_w(app)).to_be(360)
	expect(sidebar_shows(app)).to_be(true)
	expect(expanded(app, "a1")).to_be(false)
	app:set_state("inset_split_trees", { row(PAIR) })
	app:frames(60)
	expect(panel_w(app)).to_be(two)
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

test("its docked tree floats before the panes go under their least", function()
	local app = split(1440, row(PAIR))
	toggle(app)
	app:click("#panel-open-files")
	app:frames(40)
	expect_panes_keep_their_least(app, PAIR, "floating")
	expect(panel_w(app)).to_be(360)
	app:resize(1800, 800)
	app:frames(40)
	expect(panel_w(app) >= 648).to_be(true)
	expect_panes_keep_their_least(app, PAIR, "docked")
end)

test("opened by anything else: Files' key closes the sidebar too", function()
	local app = split(1100, row(PAIR))
	app:command("shell.toggle_files_tab")
	app:frames(60)
	expect(panel_w(app)).to_be(360)
	expect(sidebar_shows(app)).to_be(false)
end)

test("Expand is only ever the person's, and the sidebar opened over it closes nothing", function()
	local app = split(1100, row(TRIO))
	toggle(app)
	expect(expanded(app, "a1")).to_be(false)
	app:click("#panel-expand")
	app:frames(60)
	expect(expanded(app, "a1")).to_be(true)
	toggle_sidebar(app)
	expect(sidebar_shows(app)).to_be(true)
	expect(expanded(app, "a1")).to_be(true)
	expect(panel_w(app) > 500).to_be(true)
end)

test("another session: nothing closes on arriving, each split narrowing as it must", function()
	local app = split(1440, row(PAIR))
	toggle(app)
	expect(sidebar_shows(app)).to_be(true)
	-- The panel's another chat's; a split of three shown there, the panel open.
	app:set_state("inset_split_trees", { row(TRIO) })
	app:frames(60)
	expect(sidebar_shows(app)).to_be(true)
	expect(panel_w(app)).to_be(360)
	expect(app:has("#split-pane-a3")).to_be(true)
end)

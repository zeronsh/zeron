-- The sidebar's rows take new places smoothly: a group or a session going
-- first glides there from where it stood (each row the same node, focus
-- kept), a row leaving closes where it was, and with motion reduced it all
-- lands at once. On the shell in ui/shell_fixture.luau.

local SIDE = "#shell-sidebar"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	return app
end

-- A row's line in the tree, by its label.
local function line(app: any, label: string): string?
	for l in app:tree(SIDE):gmatch("[^\n]+") do
		if l:find(` button "{label}"`, 1, true) then
			return l
		end
	end
	return nil
end

local function top(app: any, label: string): number
	local l = line(app, label)
	if not l then
		error(`no "{label}" in the sidebar`)
	end
	return tonumber(l:match(" %-?%d+,(%-?%d+) ")) :: number
end

local function id(app: any, label: string): string
	return ((line(app, label) or ""):match("@(%d+)")) or "none"
end

test("a group going first glides there, its rows the same nodes, focus kept", function()
	local app = open()
	-- Answer the review chosen (focused), Borealis under Atlas.
	app:click(`{SIDE} >> role:button "Answer the review"`)
	app:frames(10)
	local from = top(app, "Borealis")
	local header, review = id(app, "Borealis"), id(app, "Answer the review")
	expect((line(app, "Answer the review") :: string)).to_contain("focused")

	app:command("borealis_moves")
	app:frames(3)
	-- On its way: between where it stood and where it goes, moving up.
	local early = top(app, "Borealis")
	app:frames(3)
	local later = top(app, "Borealis")
	app:frames(30)
	local to = top(app, "Borealis")
	expect(to < from).to_be(true)
	expect(`early {early} in ({to}, {from})`).to_be(`early {math.clamp(early, to + 1, from - 1)} in ({to}, {from})`)
	expect(later < early).to_be(true)
	expect(later > to).to_be(true)
	-- The same rows, focus where it was.
	expect(`borealis @{id(app, "Borealis")}`).to_be(`borealis @{header}`)
	expect(`review @{id(app, "Answer the review")}`).to_be(`review @{review}`)
	expect((line(app, "Answer the review") :: string)).to_contain("focused")
end)

test("a row leaving closes where it was, those under it closing up", function()
	local app = open()
	app:command("crowd_atlas")
	app:command("atlas_asks")
	app:frames(40)
	-- Atlas 6 asks, so it shows past Atlas's five; answered, it goes.
	local pager_was = top(app, "Show 3 more")
	app:command("atlas_answered")
	app:frames(2)
	-- Still there, closing; Show more partway up; ⌘1–9 past it already.
	expect(line(app, "Atlas 6")).never.to_be_nil()
	local mid = top(app, "Show 4 more")
	expect(mid < pager_was).to_be(true)
	expect(table.find((app:state("shell_sidebar_order") :: any), "x6")).to_be_nil()
	app:frames(30)
	expect(line(app, "Atlas 6")).to_be_nil()
	local pager_now = top(app, "Show 4 more")
	expect(mid > pager_now).to_be(true)
end)

test("with motion reduced, a group going first is there at once", function()
	local app = open()
	app:set_state("motion_reduce", true)
	app:frames(2)
	app:command("borealis_moves")
	app:frames(2)
	local landed = top(app, "Borealis")
	app:frames(30)
	expect(top(app, "Borealis")).to_be(landed)
	expect(landed < top(app, "Atlas")).to_be(true)
end)

-- A row's y through `n` frames (one a frame), as `act` folds or opens.
local function track(app: any, scope: string, label: string, act: () -> (), n: number): { number }
	act()
	local ys = {}
	for _ = 1, n do
		app:frames(1)
		local y = -1
		for l in app:tree(scope):gmatch("[^\n]+") do
			if l:find(` button "{label}"`, 1, true) then
				y = tonumber(l:match(" %-?%d+,(%-?%d+) ")) :: number
				break
			end
		end
		expect(`{label} found {y ~= -1}`).to_be(`{label} found true`)
		table.insert(ys, y)
	end
	return ys
end
-- One clean move: from where it was to where it ends, never past it, and
-- still once there (no bounce, no settle after it lands).
local function clean(ys: { number }, what: string)
	local final = ys[#ys]
	-- (It moved: the fold did something.)
	expect(`{what} moved {ys[1] ~= final or #ys < 2}`).to_be(`{what} moved true`)
	local dir = 0
	for i = 2, #ys do
		local step = ys[i] - ys[i - 1]
		if step ~= 0 then
			local d = if step > 0 then 1 else -1
			if dir ~= 0 and d ~= dir then
				expect(`{what} turned back at frame {i}: {table.concat(ys, " ")}`).to_be(`{what} one way`)
			end
			dir = d
		end
	end
	local landed = nil
	for i, y in ys do
		if y == final then
			landed = landed or i
		else
			landed = nil
		end
	end
	expect(`{what} lands and stays ({table.concat(ys, " ")})`).to_be(`{what} lands and stays ({table.concat(ys, " ")})`)
	expect((landed or #ys + 1) <= #ys - 10).to_be(true)
end

test("a group folds and opens in one motion: the rows under it never pass their place and don't move once there", function()
	local app = open()
	local function click()
		app:click(`{SIDE} >> role:button "Atlas"`)
	end
	clean(track(app, SIDE, "Borealis", click, 45), "close")
	clean(track(app, SIDE, "Borealis", click, 45), "open")
	-- An inner group's fold (project, then device): the same.
	app:command("atlas_on_box")
	app:set_state("screens/shell/sidebar.luau::shell_sidebar_organize", { "project", "device" })
	app:frames(40)
	local function inner()
		app:click(`{SIDE} >> role:button "MacBook Pro"`)
	end
	clean(track(app, SIDE, "Borealis", inner, 45), "inner close")
	clean(track(app, SIDE, "Borealis", inner, 45), "inner open")
	-- Reduce motion: there at once, and still.
	app:set_state("motion_reduce", true)
	app:frames(2)
	local ys = track(app, SIDE, "Borealis", click, 20)
	expect(ys[2]).to_be(ys[#ys])
end)

test("the panel home's parts fold in one motion too", function()
	local app = mount("panel_home_fixture.luau")
	app:resize(1000, 900)
	app:frames(20)
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	app:frames(10)
	local function click()
		app:click("#panel-home-side-head")
	end
	clean(track(app, "#panel-home-lanes", "Subagents", click, 45), "panel close")
	clean(track(app, "#panel-home-lanes", "Subagents", click, 45), "panel open")
end)

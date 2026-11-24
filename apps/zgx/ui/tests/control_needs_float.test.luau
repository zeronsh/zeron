-- Needs you floating on Map, Chat and Notes (screens/control/page
-- `NeedsFloat`): what waits on the person, answerable from any tab.
-- Compact (its count and the top ask, the rest behind "n more"), Needs
-- you's own answers and actions (screens/control/needs); never on the
-- Board (it has the asks as cards), never while nothing waits, never built
-- while it doesn't show; clear of Capture's bar. On
-- ui/control_page_fixture.luau (ui/control_world, whose sample has asks
-- waiting).

local FLOAT = "#control-needs-float"

local function open(w: number?): any
	local app = mount("control_page_fixture.luau")
	app:resize(w or 1440, 900)
	app:frames(30)
	return app
end

local function tab(app: any, id: string)
	app:set_state("control_tab", id)
	app:frames(30)
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

-- Its box: x, y, w, h.
local function box(app: any, target: string): (number, number, number, number)
	local line = string.match(app:tree(target, 0), "[^\n]+") or ""
	local x, y, w, h = string.match(line, " (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
	return tonumber(x) or 0, tonumber(y) or 0, tonumber(w) or 0, tonumber(h) or 0
end

test("needs float: on Map, Chat and Notes while something waits, never on the Board", function()
	local app = open()
	-- (The Map, the page's first tab.)
	expect(app:state("control_tab")).to_be("map")
	expect(app:has(FLOAT)).to_be(true)
	tab(app, "board")
	app:frames(20)
	expect(app:has(FLOAT)).to_be(false)
	tab(app, "map")
	expect(app:has(FLOAT)).to_be(true)
	tab(app, "chat")
	expect(app:has(FLOAT)).to_be(true)
	tab(app, "notes")
	expect(app:has(FLOAT)).to_be(true)
	tab(app, "board")
	app:frames(20)
	-- (Gone, not hidden: nothing of it built on the Board.)
	expect(app:has(FLOAT)).to_be(false)
	expect(app:has("#control-needs-pill")).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("needs float: compact, its count and the top ask, the rest behind n more", function()
	local app = open()
	tab(app, "map")
	expect(app:count(`{FLOAT} >> .orch-need`)).to_be(1)
	local more = app:has(`{FLOAT} >> ~"more"`)
	expect(more).to_be(true)
	app:click(`{FLOAT} >> role:button ~"more"`)
	app:frames(20)
	expect(app:count(`{FLOAT} >> .orch-need`) > 1).to_be(true)
end)

test("needs float: a chip answers the ask", function()
	local app = open()
	app:command("fixture.decision")
	app:frames(20)
	tab(app, "map")
	-- (The top ask's chips: the urgent one first.)
	local chip = `{FLOAT} >> .orch-choice [1]`
	expect(app:has(chip)).to_be(true)
	app:click(chip)
	app:frames(4)
	expect(app:has(`{FLOAT} >> .orch-sending`)).to_be(true)
	expect(app:wait(`{FLOAT} >> role:button "Undo"`, { gone = true, timeout_ms = 8000 })).to_be(true)
	app:frames(4)
	expect(calls(app)).to_contain("answer:")
end)

test("needs float: nothing waiting, nothing at all", function()
	local app = open()
	tab(app, "map")
	expect(app:has(FLOAT)).to_be(true)
	-- Every ask answered, no card in the board's Needs you (sccache's
	-- blocked one picked up again) and no chat awaiting input.
	local n = table.clone(app:state("orchestration") :: any)
	n.asks = {}
	local items = {}
	for _, it in n.items or {} do
		local it2 = table.clone(it)
		if it2.state == "blocked" or it2.state == "review" then
			it2.state = "active"
		end
		table.insert(items, it2)
	end
	n.items = items
	app:set_state("orchestration", n)
	local s: any = table.clone(app:state("sessions") :: any)
	local function idle(list: { any }?): { any }
		local out = {}
		for _, c in list or {} do
			local c2 = table.clone(c)
			if c2.status == "input" or c2.status == "approval" then
				c2.status = "idle"
			end
			table.insert(out, c2)
		end
		return out
	end
	s.chats = idle(s.chats)
	local kids = {}
	for k, l in s.children or {} do
		kids[k] = idle(l)
	end
	s.children = kids
	app:set_state("sessions", s)
	app:frames(40)
	expect(app:has(FLOAT)).to_be(false)
end)

test("needs float: clear of Capture's bar, wide and narrow, on every tab it shows on", function()
	for _, w in { 1440, 900 } do
		local app = open(w)
		for _, t in { "map", "chat", "notes" } do
			tab(app, t)
			app:frames(20)
			local ax, ay, aw, ah = box(app, FLOAT)
			local bx, by, bw, bh = box(app, "#control-float")
			expect(aw > 0 and bw > 0).to_be(true)
			local apart = ax + aw <= bx or bx + bw <= ax or ay + ah <= by or by + bh <= ay
			expect(apart).to_be(true)
		end
	end
end)

test("needs float: minimised to its count, opened again by a click", function()
	local app = open()
	tab(app, "map")
	app:click("#control-needs-min")
	app:frames(20)
	expect(app:has(FLOAT)).to_be(false)
	expect(app:has("#control-needs-pill")).to_be(true)
	expect(app:has('#control-needs-pill >> ~"need you"')).to_be(true)
	app:click("#control-needs-pill")
	app:frames(20)
	expect(app:has(FLOAT)).to_be(true)
	expect(app:has("#control-needs-pill")).to_be(false)
end)

test("needs float: minimised, an ask it hasn't seen opens it again", function()
	local app = open()
	tab(app, "map")
	app:click("#control-needs-min")
	app:frames(20)
	expect(app:has("#control-needs-pill")).to_be(true)
	app:command("fixture.decision")
	app:frames(30)
	expect(app:has(FLOAT)).to_be(true)
	expect(app:has("#control-needs-pill")).to_be(false)
end)

test("needs float: none over the Board, its last cards scrolled clear of Capture's bar", function()
	local app = open()
	app:resize(1440, 500)
	tab(app, "board")
	app:frames(30)
	app:scroll("#control-board-scroll", 5000)
	app:frames(30)
	expect(app:has(FLOAT)).to_be(false)
	local _, fy = box(app, "#control-float")
	-- (The Done column's foot, its last thing.)
	local _, cy, _, ch = box(app, "#orch-col-done")
	expect(cy + ch <= fy).to_be(true)
end)

test("needs float: a toast in its corner stands over it, the float where it was", function()
	local app = open()
	-- (Floating over the Map: not on the Board.)
	tab(app, "map")
	expect(app:has(FLOAT)).to_be(true)
	local _, fy, _, _ = box(app, FLOAT)
	app:command("say_toast")
	app:frames(40)
	expect(app:has('role:status >> "Marked done"')).to_be(true)
	local _, ty, _, th = box(app, 'role:status')
	-- (The toast's foot over the float's top; the float not moved.)
	expect(ty + th <= fy).to_be(true)
	local _, fy2, _, _ = box(app, FLOAT)
	expect(fy2).to_be(fy)
end)

test("needs float: Capture's history opened on a narrow page stops short of it, on Map, Chat and Notes", function()
	-- (A 1440 window's page beside the sidebar.)
	local app = open(1170)
	local capped = 0
	for _, t in { "map", "chat", "notes" } do
		tab(app, t)
		expect(app:has(FLOAT)).to_be(true)
		app:click("#control-history-open")
		app:frames(30)
		expect(app:has("#control-history")).to_be(true)
		local hx, hy, hw, hh = box(app, "#control-history")
		local nx, ny, nw, nh = box(app, FLOAT)
		local apart = hx + hw <= nx or nx + nw <= hx or hy + hh <= ny or ny + nh <= hy
		expect(`{t} apart {apart}`).to_be(`{t} apart true`)
		capped = hw
		app:click("#control-history-open")
		app:frames(30)
	end
	-- (On the Board, no Needs you floating: the panel wider again.)
	tab(app, "board")
	app:click("#control-history-open")
	app:frames(30)
	local _, _, bw = box(app, "#control-history")
	expect(bw > capped).to_be(true)
	expect(app:error()).to_be_nil()
end)

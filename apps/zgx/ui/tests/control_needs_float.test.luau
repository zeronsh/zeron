-- Needs you floating on Board and Chat (screens/control/page `NeedsFloat`):
-- what waits on the person, answerable without going back to Overview.
-- Compact (its count and the top ask, the rest behind "n more"), the
-- widget's own answers and actions; never on Overview (the widget is
-- there), never while nothing waits, never built while it doesn't show;
-- clear of the floating composer. On ui/control_page_fixture.luau
-- (ui/control_world, whose sample has asks waiting).

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

test("needs float: on Board and Chat while something waits, never on Overview", function()
	local app = open()
	expect(app:has(FLOAT)).to_be(false)
	tab(app, "board")
	expect(app:has(FLOAT)).to_be(true)
	tab(app, "chat")
	expect(app:has(FLOAT)).to_be(true)
	tab(app, "overview")
	app:frames(20)
	-- (Gone, not hidden: nothing of it built on Overview.)
	expect(app:has(FLOAT)).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("needs float: compact, its count and the top ask, the rest behind n more", function()
	local app = open()
	tab(app, "board")
	expect(app:count(`{FLOAT} >> .orch-need`)).to_be(1)
	local more = app:has(`{FLOAT} >> ~"more"`)
	expect(more).to_be(true)
	app:click(`{FLOAT} >> role:button ~"more"`)
	app:frames(20)
	expect(app:count(`{FLOAT} >> .orch-need`) > 1).to_be(true)
end)

test("needs float: a chip answers the ask as the widget's does", function()
	local app = open()
	app:command("fixture.decision")
	app:frames(20)
	tab(app, "board")
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
	tab(app, "board")
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

test("needs float: clear of the floating composer, wide and narrow", function()
	for _, w in { 1440, 900 } do
		local app = open(w)
		tab(app, "board")
		app:frames(20)
		local ax, ay, aw, ah = box(app, FLOAT)
		local bx, by, bw, bh = box(app, "#control-float")
		local apart = ax + aw <= bx or bx + bw <= ax or ay + ah <= by or by + bh <= ay
		expect(apart).to_be(true)
	end
end)

test("needs float: minimised to its count, opened again by a click", function()
	local app = open()
	tab(app, "board")
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
	tab(app, "board")
	app:click("#control-needs-min")
	app:frames(20)
	expect(app:has("#control-needs-pill")).to_be(true)
	app:command("fixture.decision")
	app:frames(30)
	expect(app:has(FLOAT)).to_be(true)
	expect(app:has("#control-needs-pill")).to_be(false)
end)

test("needs float: the Board keeps room at its end, its last cards scrolled clear of it", function()
	local app = open()
	app:resize(1440, 500)
	tab(app, "board")
	app:frames(30)
	app:scroll("#control-board-scroll", 5000)
	app:frames(30)
	local _, fy = box(app, FLOAT)
	-- (The Done column's foot, its last thing.)
	local _, cy, _, ch = box(app, "#orch-col-done")
	expect(cy + ch <= fy).to_be(true)
end)

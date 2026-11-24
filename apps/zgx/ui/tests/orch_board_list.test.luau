-- The board's list view on a busy day (ui/orch_board_list_fixture.luau, as
-- the Control room's Board showed it): each row on its own surface with a
-- gap between, its expand control in its head line and no empty band, and
-- the engine's one ask for an orchestrator's open PRs ("12 PRs to review",
-- standing on every review card) said once, at the head of Needs you,
-- never under the review cards.

-- A node's box as the tree says it.
local function box(app: any, sel: string): { x: number, y: number, w: number, h: number }
	local first = string.match(app:tree(sel, 0), "[^\n]+") or ""
	local x, y, w, h = string.match(first, " (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
	return { x = tonumber(x) or 0, y = tonumber(y) or 0, w = tonumber(w) or 0, h = tonumber(h) or 0 }
end

-- Every box of the nodes a tree's lines name (`pattern` on a line).
local function boxes(app: any, sel: string, pattern: string): { { id: string, x: number, y: number, w: number, h: number } }
	local out = {}
	for line in string.gmatch(app:tree(sel, 12), "[^\n]+") do
		local id = string.match(line, pattern)
		if id then
			local x, y, w, h = string.match(line, " (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
			if x then
				table.insert(out, { id = id, x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number })
			end
		end
	end
	return out
end

local function list(w: number?): any
	local app = mount("orch_board_list_fixture.luau")
	app:resize(w or 2024, 1102)
	app:frames(30)
	expect(app:error()).to_be_nil()
	return app
end

local REVIEWS = 7

test("orch board list: every row stands on its own surface, apart from the next", function()
	local app = list()
	local rows = app:count("#orch-list >> .orch-list-row")
	expect(rows > 10).to_be(true)
	-- Each row's head line is inside a surface of its own.
	expect(app:count("#orch-list >> .orch-list-item")).to_be(rows)
	local items = boxes(app, "#orch-list-needs_you", "#orch%-item%-([%w%-]+) ")
	expect(#items).to_be(3 + REVIEWS + 1)
	for i = 2, #items do
		local above, below = items[i - 1], items[i]
		local gap = below.y - (above.y + above.h)
		-- A visible gap, small: dense still.
		if gap < 2 or gap > 8 then
			expect(`{above.id} -> {below.id}: gap {gap}`).to_be("a gap of 2 to 8")
		end
		expect(below.x).to_be(above.x)
		expect(below.w).to_be(above.w)
	end
end)

test("orch board list: the expand control is in the head line, and no row has an empty band", function()
	local app = list()
	for _, id in { "it-plugin", "it-1474", "it-deploy", "it-usb" } do
		local line = box(app, `#orch-row-{id}`)
		expect(app:has(`#orch-row-{id} >> .orch-list-expand`)).to_be(true)
		local ex = box(app, `#orch-row-{id} >> .orch-list-expand`)
		-- Within the head line's band, at its end.
		expect(ex.y >= line.y and ex.y + ex.h <= line.y + line.h).to_be(true)
		expect(line.x + line.w - (ex.x + ex.w) < 16).to_be(true)
		-- No expand control on a line of its own past the buttons.
		expect(app:has(`#orch-item-{id} >> .orch-choice-expand`)).to_be(false)
		-- The surface ends where its last line (the buttons) does, past an
		-- even inset.
		local item = box(app, `#orch-item-{id}`)
		local last = 0
		for _, c in boxes(app, `#orch-item-{id}`, '(button) "[^"]+" ') do
			if c.y > line.y + line.h - 1 then
				last = math.max(last, c.y + c.h)
			end
		end
		expect(last > 0).to_be(true)
		local band = item.y + item.h - last
		if band > 12 then
			expect(`{id}: {band} under its buttons`).to_be("no empty band")
		end
	end
	-- A row with no ask of its own is its head line alone.
	for i = 1, REVIEWS do
		local item = box(app, `#orch-item-it-review-{i}`)
		local line = box(app, `#orch-row-it-review-{i}`)
		expect(item.h - line.h <= 2).to_be(true)
		expect(app:has(`#orch-item-it-review-{i} >> .orch-list-expand`)).to_be(false)
	end
	-- Every head line the same height, with an expand control or a chip.
	local h = box(app, "#orch-row-it-plugin").h
	expect(box(app, "#orch-row-it-review-1").h).to_be(h)
	expect(box(app, "#orch-row-it-chat").h).to_be(h)
end)

test("orch board list: no review row says the shared PR ask; it shows once, at the head of Needs you", function()
	local app = list()
	for i = 1, REVIEWS do
		local t = app:tree(`#orch-item-it-review-{i}`, 8)
		expect(string.find(t, "12 PRs to review", 1, true)).to_be_nil()
		expect(app:has(`#orch-item-it-review-{i} >> .orch-list-question`)).to_be(false)
	end
	expect(app:count("#orch-list >> .orch-list-shared")).to_be(1)
	-- (One row says it: its head line, and the text in it.)
	local n = 0
	for _ in string.gmatch(app:tree("#orch-list", 14), "12 PRs to review") do
		n += 1
	end
	expect(n <= 2).to_be(true)
	expect(box(app, "#orch-list-ask-prs-orc-fixes").y < box(app, "#orch-item-it-plugin").y).to_be(true)
	-- Its expand control opens it whole.
	app:click("#orch-list-ask-prs-orc-fixes >> .orch-list-expand")
	app:frames(5)
	expect(string.find(app:state("calls") :: string, "expand:prs-orc-fixes", 1, true) ~= nil).to_be(true)
	-- Its cards still need the person, and still say why: their PR.
	expect(string.find(app:tree("#orch-row-it-review-1", 4), "PR #1570", 1, true) ~= nil).to_be(true)
	-- Running's tab: no Needs you, no shared row.
	app:set_state("board_tab", "running")
	app:frames(10)
	local shown = false
	for line in app:tree("#orch-list", 2):gmatch("[^\n]+") do
		if string.find(line, "#orch-list-needs_you ", 1, true) then
			shown = string.find(line, " hidden", 1, true) == nil
		end
	end
	expect(shown).to_be(false)
end)

test("orch board list: a card's own ask still shows its question and buttons", function()
	local app = list()
	expect(app:has("#orch-item-it-1474 >> .orch-list-question")).to_be(true)
	expect(app:has('#orch-item-it-1474 >> role:button "Continue"')).to_be(true)
	expect(app:has('#orch-item-it-1474 >> role:button "Close it"')).to_be(true)
	expect(app:has('#orch-item-it-1474 >> role:button "Drop it"')).to_be(true)
	app:click("#orch-row-it-1474 >> .orch-list-expand")
	app:frames(5)
	expect(string.find(app:state("calls") :: string, "expand:ak-stall", 1, true) ~= nil).to_be(true)
end)

test("orch board list: a long question wraps over two lines inside its row", function()
	for _, w in { 2024, 900 } do
		local app = list(w)
		local item = box(app, "#orch-item-it-plugin")
		local q = box(app, "#orch-item-it-plugin >> .orch-list-question")
		local line = box(app, "#orch-row-it-plugin")
		-- Two lines (its height near two of the head line's words).
		expect(q.h >= 28).to_be(true)
		expect(q.x + q.w <= item.x + item.w).to_be(true)
		expect(q.y + q.h <= item.y + item.h).to_be(true)
		expect(q.y >= line.y + line.h).to_be(true)
		-- Its buttons under it, inside the row too.
		local b = box(app, '#orch-item-it-plugin >> role:button "Not now"')
		expect(b.y >= q.y + q.h).to_be(true)
		expect(b.y + b.h <= item.y + item.h).to_be(true)
	end
end)

test("orch board list: the board's cards don't say the shared ask either", function()
	local app = list()
	app:set_state("board_view", "board")
	app:frames(30)
	expect(app:error()).to_be_nil()
	expect(string.find(app:tree("#orch-card-it-review-1", 10), "12 PRs to review", 1, true)).to_be_nil()
	-- A card's own question still is its summary.
	expect(string.find(app:tree("#orch-card-it-1474", 10), "Nothing is moving on", 1, true) ~= nil).to_be(true)
end)

test("orch board list: a review card's detail keeps the shared PR ask as context, with its pull requests", function()
	local app = list()
	app:click("#orch-row-it-review-1")
	app:frames(30)
	expect(app:has("#orch-board-detail")).to_be(true)
	-- Its question and its pull requests, in the detail's box.
	expect(app:text("#orch-detail-question")).to_be("12 PRs to review")
	expect(app:count("#orch-detail-needs >> .orch-detail-ask-link")).to_be(REVIEWS)
	expect(app:has('#orch-detail-needs >> role:button "PR #1570"')).to_be(true)
	app:click('#orch-detail-needs >> role:button "PR #1503"')
	app:frames(5)
	expect(string.find(app:state("calls") :: string, "link:PR #1503", 1, true) ~= nil).to_be(true)
	-- Never on the card's face: its row stays its head line alone.
	expect(string.find(app:tree("#orch-item-it-review-1", 8), "12 PRs to review", 1, true)).to_be_nil()
	-- A card whose ask is its own shows no such list.
	app:click("#orch-row-it-1474")
	app:frames(30)
	expect(app:has("#orch-detail-needs >> .orch-detail-ask-link")).to_be(false)
end)

test("orch board list: the shared ask counts one in Needs you's section, its tab and its column", function()
	local app = list()
	-- Eleven cards and the shared ask.
	local head = app:tree('#orch-list-needs_you >> role:button "Needs you"', 2)
	expect(head).to_contain('"12"')
	expect(app:tree('#orch-board-tabs >> role:tab "Needs you"', 2)).to_contain('"12"')
	app:set_state("board_view", "board")
	app:frames(30)
	expect(app:text("#orch-col-needs_you >> .orch-column-count")).to_be("12")
	-- Said once in the column too, at its head.
	expect(app:count("#orch-col-needs_you >> .orch-list-shared")).to_be(1)
end)

-- Board cards fit their box at any column width (ds/orch_board): the
-- quiet line truncates and its PR chips drop to a line of their own when
-- there's no room; Review's buttons wrap; nothing of a card past its edge.
-- On ui/control_page_fixture.luau (ui/control_world), every card variant
-- there: Review with its buttons, PR chips beside progress, long titles,
-- outcomes, Blocked, a decision's chips.

local function boxes(app: any, target: string): { { x: number, y: number, w: number, h: number, line: string } }
	local out = {}
	for line in app:tree(target, 40, 4000):gmatch("[^\n]+") do
		if not string.find(line, " hidden", 1, true) then
			local x, y, w, h = string.match(line, " (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
			if x then
				table.insert(out, { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number, line = line })
			end
		end
	end
	return out
end

-- The person's cards (a Review card with a PR beside its progress, a long
-- title, an outcome, its buttons; a Done card with links), added to the
-- sample's.
local function crowded(app: any)
	local n = table.clone(app:state("orchestration") :: any)
	local items = table.clone(n.items or {})
	local orch = (items[1] or {}).orchestrator_id
	local now = os.time()
	table.insert(items, {
		id = "it-fit-review", orchestrator_id = orch, state = "review", priority = "high", progress = 1, order = 1,
		title = "Changes panel diff: horizontal scroll with fixed gutter and a title long enough to clamp",
		outcome = "Review: PR https://github.com/zeronsh/zeron/pull/1402 A crew thread is now working on the gutter",
		links = { { kind = "pr", label = "#1402", target = "https://github.com/zeronsh/zeron/pull/1402" } },
		updated = now - 77 * 3600,
	})
	table.insert(items, {
		id = "it-fit-done", orchestrator_id = orch, state = "done", priority = "urgent", order = 2, done = now - 60, updated = now - 60,
		title = "EMERGENCY: app not responding + lag everywhere",
		outcome = "Done. - Live: the ANR fixes. The Control Room main thread no longer spins",
		links = { { kind = "pr", label = "#2265", target = "https://github.com/zeronsh/zgx/pull/2265" }, { kind = "pr", label = "#2269", target = "https://github.com/zeronsh/zgx/pull/2269" } },
	})
	n.items = items
	app:set_state("orchestration", n)
end

-- A Board at about this column width: its window.
local function board(col: number): any
	local app = mount("control_page_fixture.luau")
	app:set_state("control_tab", "board")
	-- (Five columns side by side, the page's sides and gaps around them.)
	app:resize(col * 5 + 95, 2400)
	app:frames(60)
	crowded(app)
	app:frames(120)
	app:set_state("control_board_expanded", true)
	app:frames(60)
	return app
end

for _, col in { 200, 220, 260, 320 } do
	test(`board fit: every card's contents inside its box, columns about {col} wide`, function()
		local app = board(col)
		local cards = app:count("#control-board-tab >> .orch-card")
		expect(cards > 5).to_be(true)
		expect(app:has('#control-board-review >> .orch-card "Changes panel diff: horizontal scroll with fixed gutter and a title long enough to clamp"')).to_be(true)
		local col_line = string.match(app:tree("#control-board-review", 0), "[^\n]+") or ""
		print(`COLUMN {col}: {col_line}`)
		local outside: { string } = {}
		for i = 1, cards do
			local target = `#control-board-tab >> .orch-card [{i}]`
			local all = boxes(app, target)
			local card = all[1]
			for j = 2, #all do
				local b = all[j]
				if b.w > 0 and b.h > 0 and (b.x < card.x - 0.5 or b.x + b.w > card.x + card.w + 0.5) then
					table.insert(outside, `{card.line} ⟶ {b.line}`)
				end
			end
		end
		if #outside > 0 then
			print(table.concat(outside, "\n", 1, math.min(#outside, 8)))
		end
		expect(#outside).to_be(0)
		expect(app:error()).to_be_nil()
	end)
end

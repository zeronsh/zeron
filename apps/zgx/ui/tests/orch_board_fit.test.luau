-- Board cards fit their box at any column width (ds/orch_board,
-- ds/orch_board_card): a long title clamps to two lines, the summary to
-- two, the foot's words and clock truncate before its threads and chip;
-- nothing of a card past its edge. On ui/control_page_fixture.luau's Board
-- tab (ui/control_world's sample), with long cards added: one at work with
-- a long note and a pull request's chip, one done today with a merge's.

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

local LONG = "Changes panel diff: horizontal scroll with fixed gutter and a title long enough to clamp"

-- The long cards, added to the sample's, with the engine's rows for them.
local function crowded(app: any)
	local n = table.clone(app:state("orchestration") :: any)
	local items = table.clone(n.items or {})
	local cards = table.clone(n.cards or {})
	local orch = (items[1] or {}).orchestrator_id
	local now = os.time()
	table.insert(items, {
		id = "it-fit-work", orchestrator_id = orch, state = "active", priority = "high", order = 1, threads = { "th-fit-a", "th-fit-b" },
		title = LONG, detail = "Review: PR https://github.com/zeronsh/zeron/pull/1402 A crew thread is now working on the gutter, and then on the rest of it",
		links = {}, created = now - 3 * 3600, updated = now - 60,
	})
	table.insert(cards, {
		id = "it-fit-work", place = "running", status = "waiting", note = "the review of a very long pull request title that goes on", old_state = "active",
		since = now - 3 * 3600, status_since = now - 77 * 60, asks = {},
		chip = { kind = "pr", label = "PR #1402 changes panel gutter", target = "https://github.com/zeronsh/zeron/pull/1402" },
	})
	table.insert(items, {
		id = "it-fit-done", orchestrator_id = orch, state = "done", priority = "urgent", order = 2, threads = { "th-fit-c" }, done = now - 60, updated = now - 60, created = now - 26 * 3600,
		title = "EMERGENCY: app not responding + lag everywhere, the Control Room main thread spins",
		outcome = "Done. - Live: the ANR fixes. The Control Room main thread no longer spins, and the sidebar's rows update alone",
		links = {},
	})
	table.insert(cards, {
		id = "it-fit-done", place = "done", status = "done", old_state = "done", since = now - 60, status_since = now - 60, asks = {}, unseen = true,
		closed = { at = now - 60, by = "engine", how = "merged" },
		chip = { kind = "merged", label = "merged 2265abcdef0123" },
	})
	n.items = items
	n.cards = cards
	app:set_state("orchestration", n)
end

-- A Board this wide: its window.
local function board(w: number): any
	local app = mount("control_page_fixture.luau")
	app:set_state("control_tab", "board")
	app:resize(w, 2400)
	app:frames(60)
	crowded(app)
	app:frames(120)
	return app
end

-- (Stacked at 800; three side by side at about 245, 320 and 420 points.)
for _, w in { 800, 830, 1050, 1350 } do
	test(`board fit: every card's contents inside its box, the window {w} wide`, function()
		local app = board(w)
		local cards = app:count("#control-board >> .orch-card")
		expect(cards > 5).to_be(true)
		expect(app:has(`#control-board >> role:button "{LONG}"`)).to_be(true)
		local outside: { string } = {}
		for i = 1, cards do
			local target = `#control-board >> .orch-card [{i}]`
			local all = boxes(app, target)
			local card = all[1]
			for j = 2, #all do
				local b = all[j]
				if b.w > 0 and b.h > 0 and (b.x < card.x - 0.5 or b.x + b.w > card.x + card.w + 0.5) then
					table.insert(outside, `{card.line} ⟶ {b.line}`)
				end
			end
		end
		-- (What spills, said by the failure itself.)
		expect(table.concat(outside, "\n", 1, math.min(#outside, 8))).to_be("")
		-- The long title: two lines at most.
		local title = boxes(app, `#control-board >> role:button "{LONG}" >> .orch-card-title`)[1]
		expect(title.h <= 34).to_be(true)
		expect(app:error()).to_be_nil()
	end)
end

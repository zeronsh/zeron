-- The Control room (ui/control_world's sample) with Needs you as the board
-- has it: a card asking two things (one row, "1 more"), an ask that
-- belongs to no card (a row of its own), an ask put off (no row), and the
-- card's own thread awaiting input (no row of its own). For the tests of
-- Needs you's rows (tests/control_needs_cards) and their pictures.
--
-- Commands: `fx.board` / `fx.map` / `fx.chat` / `fx.notes` (the tab), `theme_dark`,
-- `theme_light` (ui/control_world's).

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
require("@app/screens/shell/route").launched:set(true)
require("@app/control_world")()
local orchestration = require("@account/orchestration")
local page = require("@app/screens/control/page")

local t = os.time()
local now: any = table.clone(orchestration.now:peek())
local asks = table.clone(now.asks or {})
-- The orchestrator screen's card asks a second thing.
table.insert(asks, {
	id = "ak-1b", from_chat = "th-ui-orch", orchestrator_id = "orc-orch", item_id = "it-orch-screen", kind = "question",
	question = "Keep the crew list under the board when the page is narrow?",
	options = { { label = "Under the board" }, { label = "In a tab" } }, urgent = false, state = "open", created = t - 120,
})
-- CI speed's orchestrator asks on no card.
table.insert(asks, {
	id = "ak-free", from_chat = "orc-ci", orchestrator_id = "orc-ci", kind = "decision",
	question = "Pause the nightly iOS lane until sccache reaches the bucket?",
	options = { { label = "Pause it" }, { label = "Keep running" } }, urgent = false, state = "open", created = t - 900,
})
-- Put off until tomorrow: no row today.
table.insert(asks, {
	id = "ak-later", from_chat = "orc-vpn", orchestrator_id = "orc-vpn", kind = "question",
	question = "Rotate the exit keys this week?", options = {}, urgent = false, state = "open", created = t - 4000,
	deferred_until = t + 86400,
})
now.asks = asks
orchestration.now:set(now)

-- `fx.shared`: two cards of the Orchestrator screen's orchestrator whose PR
-- waits for review, both carrying the engine's one ask for its open PRs
-- (`prs-orc-orch`, naming no card), as the engine's rows have them: one row
-- of its own in Needs you, never on the cards' rows.
ui.command { id = "fx.shared", title = "Two review cards sharing the PR ask", run = function()
	local n: any = table.clone(orchestration.now:peek())
	local items = table.clone(n.items or {})
	local cards = table.clone(n.cards or {})
	local list = table.clone(n.asks or {})
	local links = {}
	for i, pr in { 1570, 1571 } do
		local id = `it-review-{i}`
		local url = `https://github.com/zeronsh/zeron/pull/{pr}`
		table.insert(links, { kind = "pr", label = `PR #{pr}`, target = url })
		table.insert(items, { id = id, orchestrator_id = "orc-orch", title = `Review card {i}`, state = "review", priority = "normal", threads = {}, links = { { kind = "pr", label = `PR #{pr}`, target = url } }, order = 50 + i, created = t - 7200, updated = t - 3600 * i })
		table.insert(cards, { id = id, place = "needs_you", status = "review", old_state = "review", since = t - 3600 * i, status_since = t - 3600 * i, asks = { "prs-orc-orch" }, finish = { line = "pr", pr = url }, chip = { kind = "pr", label = `PR #{pr}`, target = url }, events = {} })
	end
	table.insert(list, {
		id = "prs-orc-orch", from_chat = "orc-orch", orchestrator_id = "orc-orch", kind = "approval", origin = "review",
		question = "12 PRs to review", options = {}, urgent = false, state = "open", created = t - 7000, links = links,
		context = "- Review card 1\n- Review card 2",
	})
	n.items, n.cards, n.asks = items, cards, list
	orchestration.now:set(n)
end }

-- The board's own menu, as a secondary click on Running's head opens it.
ui.command { id = "fx.board_menu", title = "The board's menu, from a secondary click", run = function()
	require("@ds/orch_board_actions").open_board_menu(900, 260)
end }

for _, tab in { "board", "map", "chat", "notes" } do
	ui.command { id = `fx.{tab}`, title = `Show {tab}`, run = function()
		page.tab:set(tab)
	end }
end

return ui.column {
	w_full = true,
	h_full = true,
	bg = "$background",
	page.Page(),
}

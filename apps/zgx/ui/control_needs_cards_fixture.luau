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

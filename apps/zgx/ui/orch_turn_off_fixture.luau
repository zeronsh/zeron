-- The Control room's page, with each of account/orchestration's parts read
-- on the page: for turning Orchestrators off while they're all live. Off,
-- the window sets the shared orchestration value to nil
-- (src/orchestrators.rs), and nothing reading it may fail.

ui.theme.strict("warn")
require("@account/plugins").set({ orchestrators = true })
local ds = require("@ds")
ds.setup()
require("@app/screens/shell/route").launched:set(true)
require("@app/control_world")()
local orchestration = require("@account/orchestration")
local page = require("@app/screens/control/page")

-- A card that moves, for turning off while the board moves it
-- (ds/orch_board_motion): `it-engine`'s row in Running, then the engine
-- closing it (Done, `seq` one higher, `from` its old place), each pushed
-- as the bridge pushes it.
local function card_push(row: any)
	local n = table.clone(orchestration.now:peek())
	n.cards = { row }
	orchestration.now:set(n)
end
ui.command { id = "fixture.card_row", title = "Fixture: a card's row in Running", run = function()
	card_push({ id = "it-engine", place = "running", status = "working", seq = 1 })
end }
ui.command { id = "fixture.card_move", title = "Fixture: the engine closes the card", run = function()
	card_push({ id = "it-engine", place = "done", status = "done", from = "running", seq = 2, by = "engine",
		closed = { at = os.time() - 1, by = "engine", how = "report" } })
end }
return ui.column {
	w_full = true,
	h_full = true,
	bg = "$background",
	ui.text { id = "orch-parts", function(): string
		local s = orchestration.settings:get()
		return `{#orchestration.orchestrators:get()} {#orchestration.dumps:get()} {#orchestration.asks:get()} {s.crew_idle_hours}`
	end },
	page.Page(),
}

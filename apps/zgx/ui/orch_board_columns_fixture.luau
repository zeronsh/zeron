-- The board's columns (ds/orch_board), for tests/orch_board_columns.test.luau,
-- orch_board_queued.test.luau and pictures: Queued first (Up next, and
-- Later folded until opened: the state `columns_later`), Blocked standing
-- only while it holds a card, each column ten cards at most until all are
-- shown (the state `columns_expanded`, one for them all), and Done cleared
-- at once. `!fixture.later` puts four in Later, `!fixture.queued` fills Up
-- next and Later past ten. The
-- items are the state `columns_items` (`!fixture.many` fills Now and Done
-- past ten, `!fixture.blocked` puts one in Blocked); what it's asked is
-- noted in `calls`, a delete and a restore applied as the engine would. The
-- chats a card links are the state `columns_chats`: nothing here touches
-- them. Blocked's groups: `!fixture.blocked_groups` puts two in Needs you
-- (one asked: the state `columns_asks`) and two in Paused (folded until
-- opened: the state `columns_paused`); `!fixture.checks` gives four cards
-- the engine's note, one of each kind.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local OrchBoard = require("@ds/orch_board")

local T = 1_800_040_000
local function ago(s: number): number
	return T - s
end

local said = ui.state("calls", "")
local function note(what: string)
	said:set(if said:peek() == "" then what else `{said:peek()},{what}`)
end

for _, t in { "dark", "light" } do
	ui.command { id = `theme_{t}`, title = `{t} theme`, run = function()
		ui.theme.follow_system(false)
		ui.theme.current:set(`zeron-{t}`)
	end }
end

local orchestrators = ui.state("columns_orchestrators", {
	{ id = "orc-fix", name = "Zeron fixes", hue = 18 },
	{ id = "orc-vpn", name = "Exnomic VPN", hue = 200 },
} :: { any })

local chats = ui.state("columns_chats", {
	["th-a"] = { id = "th-a", title = "Scroll resets", status = "working" },
	["th-d1"] = { id = "th-d1", title = "Titlebar", status = "done" },
} :: { [string]: any })

local function card(id: string, state: string, title: string, order: number, extra: { [string]: any }?): any
	local it: { [string]: any } = {
		id = id,
		orchestrator_id = if order % 2 == 0 then "orc-fix" else "orc-vpn",
		state = state,
		priority = "normal",
		order = order,
		title = title,
		updated = ago(600 + order * 60),
	}
	if state == "done" then
		it.done = ago(600 + order * 60)
	end
	for k, v in extra or {} do
		it[k] = v
	end
	return it
end

-- A few of each but Inbox and Blocked: those stand aside.
local BASE = {
	card("it-now1", "active", "Scroll position resets", 1, { threads = { "th-a" } }),
	card("it-now2", "active", "Fix slash menu in Codex chats", 2),
	card("it-next1", "planned", "Phase 2 migration", 1),
	card("it-next2", "planned", "Write the mesh runbook", 2),
	card("it-rev1", "review", "Progress clocks tick by the minute", 1),
	card("it-done1", "done", "Linux titlebar without controls", 1, { threads = { "th-d1" } }),
	card("it-done2", "done", "Delete cards from the Board", 2),
}
local items = ui.state("columns_items", table.clone(BASE) :: { any })
local expanded = ui.state("columns_expanded", false)
local later_open = ui.state("columns_later", false)
local paused_open = ui.state("columns_paused", false)
local asks = ui.state("columns_asks", {
	{ id = "ask-1", item_id = "it-blk1", state = "open", question = "Which region first?" },
	{ id = "ask-2", item_id = "it-blk2", state = "answered", question = "Renew now?" },
} :: { any })

local DONE_TITLES = {
	"Linux titlebar without controls", "Delete cards from the Board", "Map labels keep clear",
	"Codex slash menu", "Progress widget lists threads", "Capture takes dropped files",
	"Brain dump model pick", "Sidebar orchestrators section", "Chip menu crash", "Popup shadows",
	"Device names unique", "Route titles short",
}
local NOW_TITLES = {
	"Scroll position resets", "Fix slash menu in Codex chats", "Board columns cap", "Hover cards anchor",
	"Q&A section", "Engine proxy progress", "Crew reports", "Map island layout", "Shared browser spike",
	"CI admission tier", "VPN phase 2",
}
ui.command { id = "fixture.many", title = "Fill Now and Done", run = function()
	local list: { any } = {}
	for i, t in NOW_TITLES do
		table.insert(list, card(`it-now{i}`, "active", t, i, if i == 1 then { threads = { "th-a" } } else nil))
	end
	for i, t in DONE_TITLES do
		table.insert(list, card(`it-done{i}`, "done", t, i, if i == 1 then { threads = { "th-d1" } } else nil))
	end
	table.insert(list, card("it-next1", "planned", "Phase 2 migration", 1))
	table.insert(list, card("it-rev1", "review", "Progress clocks tick by the minute", 1))
	items:set(list)
end }
ui.command { id = "fixture.blocked", title = "Put one in Blocked", run = function()
	items:push(card("it-blk1", "blocked", "DNS failover between regions", 1))
end }
ui.command { id = "fixture.blocked_groups", title = "Fill Needs you and Paused", run = function()
	items:push(card("it-blk1", "blocked", "DNS failover between regions", 1))
	items:push(card("it-blk2", "blocked", "Wait for the cert", 2))
	items:push(card("it-pz1", "blocked", "Mesh admin app", 1, { paused_at = T - 12 * 60 }))
	items:push(card("it-pz2", "blocked", "Moscow relay", 2, { paused_at = "2027-01-15T18:06:40Z" }))
end }
ui.command { id = "fixture.checks", title = "Notes on four cards", run = function()
	local notes = {
		["it-now1"] = { kind = "stale", note = "Its threads finished 20m ago" },
		["it-rev1"] = { kind = "nothing_to_review", note = "Nothing to review" },
		["it-done1"] = { kind = "stillWorking", note = "A thread still works" },
		["it-next1"] = { kind = "moved", note = "Moved from Now: no thread" },
	}
	local list = table.clone(items:peek())
	for i, it in list do
		local n = notes[it.id]
		if n then
			local copy = table.clone(it)
			copy.check = { kind = n.kind, note = n.note, since = T - 300 }
			list[i] = copy
		end
	end
	items:set(list)
end }
ui.command { id = "fixture.later", title = "Put four in Later", run = function()
	for i, t in { "Look at the hover delay", "Try a darker map", "Remind me: renew the cert", "Someday: a CLI" } do
		items:push(card(`it-later{i}`, "inbox", t, i))
	end
end }
ui.command { id = "fixture.queued", title = "Fill Up next and Later", run = function()
	local list: { any } = {}
	for i = 1, 8 do
		table.insert(list, card(`it-up{i}`, "planned", `Up next thing {i}`, i))
	end
	for i = 1, 5 do
		table.insert(list, card(`it-l{i}`, "inbox", `Later idea {i}`, i))
	end
	table.insert(list, card("it-now1", "active", "Scroll position resets", 1, { threads = { "th-a" } }))
	items:set(list)
end }
ui.command { id = "fixture.inbox", title = "Put one in Later", run = function()
	items:push(card("it-in1", "inbox", "Look at the hover delay", 1))
end }

local function update(id: string, fields: { [string]: any })
	local list = table.clone(items:peek())
	for i, it in list do
		if it.id == id then
			local copy = table.clone(it)
			for k, v in fields do
				copy[k] = v
			end
			list[i] = copy
		end
	end
	items:set(list)
end

return ui.div { w_full = true, h_full = true, ui.scroll {
	id = "fixture-root",
	w_full = true,
	h_full = true,
	bg = "$background",
	ui.column {
		w_full = true,
		p = "$space.4",
		OrchBoard {
			id = "cols",
			items = items,
			orchestrators = orchestrators,
			show_filter = true,
			later_open = later_open,
			paused_open = paused_open,
			asks = function(): any
				return asks:get()
			end,
			expanded = expanded,
			now = function(): number
				return T
			end,
			thread_of = function(chat: string): any
				return chats:get()[chat]
			end,
			project_of = function(_chat: string): string?
				return "zeron"
			end,
			-- (As the engine: its thread made, the card in Now.)
			on_start = function(id: string, order: number?)
				note(`start:{id}:{order or ""}`)
				update(id, { state = "active", order = order or 99, threads = { `th-{id}` } })
				return nil
			end,
			on_move = function(id: string, state: string, order: number)
				note(`move:{id}:{state}:{order}`)
				update(id, { state = state, order = order })
			end,
			-- (As the engine: Blocked, paused; resumed, into its state.)
			on_pause = function(id: string)
				note(`pause:{id}`)
				update(id, { state = "blocked", paused_at = T })
			end,
			on_resume = function(id: string, state: string)
				note(`resume:{id}:{state}`)
				update(id, { state = state, paused_at = false })
			end,
			on_priority = function(id: string, p: string)
				note(`priority:{id}:{p}`)
				update(id, { priority = p })
			end,
			on_open_thread = function(chat: string)
				note(`open:{chat}`)
			end,
			on_remove = function(id: string)
				note(`remove:{id}`)
				update(id, { removed = true })
				return nil
			end,
			-- (As the engine: a new card, as it was.)
			on_restore = function(it: any)
				note(`restore:{it.id}`)
				local copy = table.clone(it)
				copy.id = `{it.id}-back`
				copy.removed = nil
				items:push(copy)
				return nil
			end,
		},
	},
}, ds.toast.Host {} }

-- Board cards of every kind side by side (ds/orch_board), for
-- tests/orch_board.test.luau and pictures: a long title from before cards
-- had short ones, short titles, todo progress real and none yet, urgent,
-- high and normal, and cards working, stalled, blocked, in review and done.
-- The open asks their threads raised are the state `cards_asks` (by chat),
-- none at first.
-- What it's asked noted in `calls`; a delete applied as the engine would.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local OrchBoard = require("@ds/orch_board")
local Progress = require("@ds/orch_progress")

-- A fixed moment, so the clocks say the same every time.
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

local orchestrators = ui.state("cards_orchestrators", {
	{ id = "orc-fix", name = "Zeron fixes", hue = 18 },
	{ id = "orc-vpn", name = "Exnomic VPN", hue = 200 },
} :: { any })

local chats: { [string]: any } = {
	["th-legacy"] = { id = "th-legacy", title = "Board delete", status = "working" },
	["th-scroll"] = { id = "th-scroll", title = "Scroll resets", status = "working" },
	["th-slash"] = { id = "th-slash", title = "Codex slash menu", status = "working" },
	["th-mig"] = { id = "th-mig", title = "Engine migration", status = "input" },
	["th-clock"] = { id = "th-clock", title = "Progress clocks", status = "done" },
	["th-dns"] = { id = "th-dns", title = "DNS failover", status = "failed" },
}
local todos: { [string]: any } = {
	["th-scroll"] = { done = 3, total = 7 },
	-- (A todo with nothing done yet: no bar.)
	["th-slash"] = { done = 0, total = 5 },
}
local progress: { [string]: any } = {
	["th-legacy"] = { state = "working", started = ago(180), last_step = ago(780), note = "Reading the board's menu." },
	["th-scroll"] = { state = "working", started = ago(1500), last_step = ago(25), steps = { done = 3, total = 7 } },
	["th-slash"] = { state = "working", started = ago(1020), last_step = ago(900), stalled = true, steps = { done = 0, total = 5 } },
	["th-mig"] = { state = "awaiting_input", started = ago(4000), last_step = ago(600) },
}

local asks = ui.state("cards_asks", {} :: { [string]: any })

local items = ui.state("cards_items", {
	{
		id = "it-legacy", orchestrator_id = "orc-fix", state = "active", priority = "normal", order = 3, threads = { "th-legacy" },
		title = "Feature request for the Board: the person wants to delete a card from its menu, with an Undo beside it for a while",
		source = { dump_id = "dp-1", route_id = "rt-1" }, updated = ago(780),
	},
	{
		id = "it-scroll", orchestrator_id = "orc-fix", state = "active", priority = "high", order = 1, threads = { "th-scroll" },
		title = "Scroll position resets on live re-renders", updated = ago(25),
	},
	{
		id = "it-slash", orchestrator_id = "orc-fix", state = "active", priority = "urgent", order = 0, threads = { "th-slash" },
		title = "Fix slash menu in Codex chats", updated = ago(900),
	},
	{ id = "it-delete", orchestrator_id = "orc-fix", state = "active", priority = "normal", order = 4, title = "Delete cards from the Board", updated = ago(3600) },
	{ id = "it-phase2", orchestrator_id = "orc-vpn", state = "planned", priority = "low", order = 1, title = "Phase 2 migration", updated = ago(7200) },
	{ id = "it-docs", orchestrator_id = "orc-vpn", state = "planned", priority = "normal", order = 2, title = "Write the mesh runbook", progress = 0.4, updated = ago(5400) },
	{
		id = "it-mig", orchestrator_id = "orc-vpn", state = "blocked", priority = "high", order = 1, threads = { "th-mig" },
		title = "Approve engine migration", updated = ago(600),
	},
	{
		id = "it-dns", orchestrator_id = "orc-vpn", state = "blocked", priority = "normal", order = 2, threads = { "th-dns" },
		title = "DNS failover between regions", updated = ago(2400),
	},
	{
		id = "it-clock", orchestrator_id = "orc-fix", state = "review", priority = "normal", order = 1, threads = { "th-clock" },
		title = "Progress clocks tick by the minute", updated = ago(1200),
		outcome = "Clocks move once a minute, on the minute; nothing ticks by the second. Tests pass.",
	},
	{
		id = "it-merged", orchestrator_id = "orc-fix", state = "done", priority = "normal", order = 1,
		title = "Linux titlebar without controls", updated = ago(3000), done = ago(3000),
		links = { { kind = "pr", label = "#1383", target = "https://example.invalid/pull/1383" } },
		outcome = "Merged.",
	},
} :: { any })

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
			id = "cards",
			items = items,
			orchestrators = orchestrators,
			show_filter = true,
			now = function(): number
				return T
			end,
			todo_of = function(chat: string): any
				return todos[chat]
			end,
			thread_of = function(chat: string): any
				return chats[chat]
			end,
			ask_of = function(chat: string): any
				return asks:get()[chat]
			end,
			project_of = function(_chat: string): string?
				return "zeron"
			end,
			progress_of = function(chat: string): any
				return progress[chat]
			end,
			-- (What the board embedded before its cards read the row.)
			progress = function(chat: string): Node?
				if not progress[chat] then
					return nil
				end
				return Progress {
					variant = "strip",
					lean = true,
					now = function(): number
						return T
					end,
					row = function(): any
						return progress[chat]
					end,
					chat = function(): any
						return chats[chat]
					end,
					on_check = function() end,
					on_open = function() end,
				}
			end,
			on_move = function(id: string, state: string, order: number)
				note(`move:{id}:{state}:{order}`)
				update(id, { state = state, order = order })
			end,
			on_approve = function(id: string)
				note(`approve:{id}`)
				update(id, { state = "done" })
			end,
			on_request_changes = function(id: string, text: string)
				note(`changes:{id}:{text}`)
				update(id, { state = "active" })
			end,
			on_priority = function(id: string, p: string)
				note(`priority:{id}:{p}`)
				update(id, { priority = p })
			end,
			on_open_thread = function(chat: string)
				note(`open:{chat}`)
			end,
			on_link = function(l: any)
				note(`link:{l.label}`)
			end,
			on_source = function(src: any)
				note(`source:{src.dump_id}`)
			end,
			on_remove = function(id: string)
				note(`remove:{id}`)
				update(id, { removed = true })
				return nil
			end,
			on_restore = function(it: any)
				note(`restore:{it.id}`)
				update(it.id, { removed = false })
				return nil
			end,
		},
	},
}, ds.toast.Host {} }

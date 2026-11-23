-- The gallery's Board page: the orchestration board (ds/orch_board) on the
-- sample, wide as the Control room shows it (every orchestrator, its
-- filter, New task), and narrow and dense as a side panel would (one
-- orchestrator's, stacked). The sample's items carry no engine rows, so
-- each card stands by its old column (ds/orch_board_model). What the
-- person does is said under the title, and the sample's rows change as the
-- engine's would.

local ds = require("@ds")
local OrchBoard = require("../ds/orch_board")

-- "Now" is this evening, so what was done this afternoon is today's.
local base = os.time()
local here = os.date("*t", base) :: any
local T = base - (here.hour * 3600 + here.min * 60 + here.sec) + 20 * 3600
local sample = require("../account/orchestration_sample").at(T)

local items = ui.state("orch_board_demo_items", sample.now.items :: { any })
local asks = ui.state("orch_board_demo_asks", (sample.now.asks or {}) :: { any })
local said = ui.state("orch_board_demo_said", "Open a card, or right-click one.")
local chats: { [string]: any } = {}
for _, c in sample.chats do
	chats[c.id] = c
end

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

local function board(extra: { [string]: any }): Node
	local props: { [string]: any } = {
		items = items,
		asks = asks,
		orchestrators = sample.now.orchestrators,
		now = function(): number
			return T
		end,
		thread_of = function(chat: string): any
			return chats[chat]
		end,
		on_open_thread = function(chat: string)
			said:set(`Open {chat}`)
		end,
		on_link = function(l: any)
			said:set(`Open {l.target}`)
		end,
		on_answer = function(ask: any, choice: any): any
			said:set(`Answered {ask.id}`)
			local list = table.clone(asks:peek())
			for i, a in list do
				if a.id == ask.id then
					local copy = table.clone(a)
					copy.state = "answered"
					list[i] = copy
				end
			end
			asks:set(list)
			return nil
		end,
		on_close = function(id: string, dropped: boolean): any
			said:set(`{if dropped then "Dropped" else "Marked done"} {id}`)
			update(id, { state = if dropped then "dropped" else "done", done = T })
			return nil
		end,
		on_reopen = function(id: string): any
			said:set(`Reopened {id}`)
			update(id, { state = "planned" })
			return nil
		end,
		on_hold = function(id: string, held: boolean): any
			said:set(`{if held then "Kept for later" else "Let start"} {id}`)
			update(id, { state = if held then "inbox" else "planned" })
			return nil
		end,
		on_pause = function(id: string): any
			said:set(`Paused {id}`)
			update(id, { state = "blocked", paused_at = T })
			return nil
		end,
		on_resume = function(id: string): any
			said:set(`Resumed {id}`)
			update(id, { state = "active", paused_at = false })
			return nil
		end,
		on_start = function(id: string): any
			said:set(`Started {id}`)
			update(id, { state = "active" })
			return nil
		end,
		on_remove = function(id: string): any
			said:set(`Deleted {id}`)
			update(id, { removed = true })
			return nil
		end,
		on_restore = function(item: any): any
			said:set(`Put back {item.id}`)
			update(item.id, { removed = false })
			return nil
		end,
	}
	for k, v in extra do
		props[k] = v
	end
	return OrchBoard(props :: any)
end

return {
	id = "orch-board",
	title = "Board",
	about = "Where every piece of work stands, in three columns: Needs you, Running and Done today, each with a drawer at its foot (Snoozed, Planned, Earlier). The engine works each card's place out; nothing is dragged. A card shows whose it is, its title, two lines of what it is doing or did, and its status with a clock. A click opens its detail beside the board; a secondary click, its menu. Tabs show a column alone; the switch shows the same places as a list. Wide, every orchestrator with a filter; narrow, one orchestrator's, stacked.",
	demo = function(): Node
		return ui.column {
			w_full = true,
			gap = "$space.6",
			ds.Text {
				function(): string
					return said:get()
				end,
				style = "caption",
				tone = "tertiary",
			},
			board {
				id = "orch-board-demo-wide",
				show_filter = true,
				on_new_task = function(text: string, orchestrator_id: string?): any
					said:set(`New task for {orchestrator_id or "whoever takes it"}: {text}`)
					return nil
				end,
			},
			ui.column {
				w = 460,
				gap = "$space.2",
				ds.Text { "One orchestrator's, narrow", style = "label", tone = "secondary" },
				board { id = "orch-board-demo-narrow", filter = "orc-orch", compact = true },
			},
		}
	end,
}

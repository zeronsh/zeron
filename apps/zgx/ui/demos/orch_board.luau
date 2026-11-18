-- The gallery's Board page: the orchestration board (ds/orch_board) on the
-- sample, wide as the Control room shows it (every orchestrator, its
-- filter, Inbox first), and narrow and dense as an orchestrator's page
-- shows its own. Cards drag between columns, their menus set a priority;
-- what's asked is said under it.

local ds = require("@ds")
local OrchBoard = require("@ds/orch_board")

-- "Now" is this evening, so what was done this afternoon is today's.
local base = os.time()
local here = os.date("*t", base) :: any
local T = base - (here.hour * 3600 + here.min * 60 + here.sec) + 20 * 3600
local original = require("@app/account/orchestration_sample")
local sample = original.at(T)

local items = ui.state("orch_board_demo_items", sample.now.items :: { any })
local said = ui.state("orch_board_demo_said", "Drag a card, or right-click one.")
local chats: { [string]: any } = {}
-- (A thread's todo from the sample as written: `at` shifts every `done`
-- key as a time, its todo's count among them.)
local todos: { [string]: any } = {}
for _, c: any in original.chats do
	todos[c.id] = c.todo
end
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
		orchestrators = sample.now.orchestrators,
		now = function(): number
			return T
		end,
		todo_of = function(chat: string): any
			return todos[chat]
		end,
		thread_of = function(chat: string): any
			return chats[chat]
		end,
		on_move = function(id: string, state: string, order: number)
			said:set(`Moved {id} to {state} at {order}`)
			update(id, { state = state, order = order })
		end,
		on_priority = function(id: string, p: string)
			said:set(`{id}: {p}`)
			update(id, { priority = p })
		end,
		on_open_thread = function(chat: string)
			said:set(`Open {chat}`)
		end,
		on_link = function(l: any)
			said:set(`Open {l.target}`)
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
	about = "Where every piece of an orchestrator's work stands: Inbox, Now, Next, Blocked, Review and Done today, the columns sharing its width evenly. Each card shows its status, progress, threads, pull request and, once there is one, its outcome in two lines; whose it is, where the board holds several orchestrators' work. Cards drag between columns and within one. Wide, every orchestrator with a filter; narrow, one orchestrator's, stacked.",
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
			board { id = "orch-board-demo-wide", show_filter = true, inbox = true },
			ui.column {
				w = 460,
				gap = "$space.2",
				ds.Text { "One orchestrator's, narrow", style = "label", tone = "secondary" },
				board { id = "orch-board-demo-narrow", filter = "orc-orch", compact = true },
			},
		}
	end,
}

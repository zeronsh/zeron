-- The board (ds/orch_board) on the orchestration sample, for
-- tests/orch_board.test.luau: what it's asked (moves, priorities, threads
-- opened, links) noted in `calls`, a move applied to the items as the
-- engine's state would come back. `filter` narrows it to an orchestrator
-- (the test sets it); `compact` is the page's denser look.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local OrchBoard = require("@ds/orch_board")
-- "Now" is today at 8 PM here, so what was done this afternoon is today's.
local base = os.time()
local here = os.date("*t", base) :: any
local T = base - (here.hour * 3600 + here.min * 60 + here.sec) + 20 * 3600
local original = require("@app/account/orchestration_sample")
local sample = original.at(T)

local said = ui.state("calls", "")
local function note(what: string)
	said:set(if said:peek() == "" then what else `{said:peek()},{what}`)
end
local items = ui.state("orch_board_items", sample.now.items :: { any })
local filter = ui.state("filter", nil :: string?)
local compact = ui.state("compact", false)

-- (The theme, for pictures: `theme_dark`, `theme_light`.)
for _, t in { "dark", "light" } do
	ui.command { id = `theme_{t}`, title = `{t} theme`, run = function()
		ui.theme.follow_system(false)
		ui.theme.current:set(`zeron-{t}`)
	end }
end

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

return ui.scroll {
	id = "fixture-root",
	w_full = true,
	h_full = true,
	bg = "$background",
	ui.column {
		w_full = true,
		p = "$space.4",
		ui.presence(function(): string
			return if compact:get() then "compact" else "regular"
		end, function(k: string): Node
			return OrchBoard {
				items = items,
				orchestrators = sample.now.orchestrators,
				filter = filter,
				show_filter = true,
				now = function(): number
					return T
				end,
				compact = k == "compact",
				todo_of = function(chat: string): any
					return todos[chat]
				end,
				thread_of = function(chat: string): any
					return chats[chat]
				end,
				on_move = function(id: string, state: string, order: number)
					note(`move:{id}:{state}:{order}`)
					update(id, { state = state, order = order })
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
			}
		end, { w_full = true }),
	},
}

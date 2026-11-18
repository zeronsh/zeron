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
local orchestrators = ui.state("orch_board_orchestrators", sample.now.orchestrators :: { any })
local filter = ui.state("filter", nil :: string?)
local compact = ui.state("compact", false)
local start_fails = ui.state("start_fails", false)

-- (The theme, for pictures: `theme_dark`, `theme_light`.)
for _, t in { "dark", "light" } do
	ui.command { id = `theme_{t}`, title = `{t} theme`, run = function()
		ui.theme.follow_system(false)
		ui.theme.current:set(`zeron-{t}`)
	end }
end

local PROJECTS: { [string]: string } = { ["orc-orch"] = "zeron", ["orc-vpn"] = "exnomic", ["orc-ci"] = "zeron", ["th-loose"] = "dotfiles" }
-- (Waiting cards, for pictures: one in a project, one queued on a machine.)
ui.command { id = "fixture.waiting", title = "Waiting cards", run = function()
	items:set({
		{ id = "it-p", orchestrator_id = "orc-orch", title = "Add --json output to tally", state = "planned", priority = "normal", order = 1, space_id = "p-tally" },
		{ id = "it-m", orchestrator_id = "orc-vpn", title = "Rotate the Moscow keys", state = "planned", priority = "high", order = 2, device_id = "dev-cloud", waiting = "Ops's crew is full: 2 of 2 working" },
		{ id = "it-a", orchestrator_id = "orc-orch", title = "Control room", state = "active", priority = "normal", order = 1, threads = { "th-ui-control" } },
	})
end }

local chats: { [string]: any } = { ["th-loose"] = { id = "th-loose", title = "Loose thread", status = "done" } }
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

return ui.div { w_full = true, h_full = true, ui.scroll {
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
				orchestrators = orchestrators,
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
				-- (Each orchestrator's project, its crew's too; a loose
				-- thread's own.)
				project_of = function(chat: string): string?
					local c = chats[chat]
					local root = if c and c.parent then c.parent else chat
					return PROJECTS[root]
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
				on_source = function(src: any)
					note(`source:{src.dump_id}`)
				end,
				-- (As the engine: kept, marked removed; put back, a new card.)
				on_remove = function(id: string)
					note(`remove:{id}`)
					update(id, { removed = true })
					return nil
				end,
				-- (Started as the engine would, a beat later: its thread made;
				-- `start_fails` set, refused.)
				on_start = function(id: string, order: number?): any
					note(`start:{id}:{order or ""}`)
					return {
						after = function(_self: any, done: (any) -> (), failed: ((any) -> ())?)
							ui.after(50, function()
								if start_fails:peek() then
									if failed then
										failed("no")
									end
									return
								end
								update(id, { state = "active", order = order or 99, threads = { `th-{id}` } })
								done({ id = id })
							end)
						end,
					}
				end,
				space_name = function(id: string): string?
					return ({ ["p-zeron"] = "zeron", ["p-tally"] = "tally" } :: { [string]: string })[id]
				end,
				device_name = function(id: string): string?
					return ({ ["dev-cloud"] = "Cloud" } :: { [string]: string })[id]
				end,
				on_restore = function(it: any)
					note(`restore:{it.id}:{it.title}`)
					local copy = table.clone(it)
					copy.id = `{it.id}-back`
					copy.removed = nil
					items:push(copy)
					return nil
				end,
			}
		end, { w_full = true }),
	},
}, ds.toast.Host {} }

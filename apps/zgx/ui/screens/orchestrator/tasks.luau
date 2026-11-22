-- An orchestrator's side panel's Tasks tab: what needs you (the first
-- three, then "n more") and its board, each in a folding section.
--
-- THE SLOT: the board's list view (Needs you, Running, Planned, Done as
-- folding sections) replaces both sections here. `M.Tasks` is the whole of
-- what the tab draws; give it the list view's sections in their place and
-- keep its props (`fold` and `on_open_card` are what the panel offers each
-- section: where it's folded, and a card opened in the panel's detail).
--
--   Tasks { of = fn -> orchestrator id, name_of = fn, open_link = fn,
--     needs = fn -> number, fold = { open = fn(key, default), set = fn(key, open) },
--     on_open_card = fn(chat, item?) }

local orchestration = require("@account/orchestration")
local sessions = require("@account/sessions")
local rows = require("@account/rows")
local OrchBoard = require("@ds/orch_board")
local Fold = require("@ds/orch_fold")
local stage = require("@ds/stage")
local locale = require("@ds/locale")
local route = require("@app/screens/shell/route")
local Progress = require("@app/screens/control/progress")
local NeedsYou = require("@app/screens/control/needs")

local M = {}

-- Needs you's rows before "n more".
M.NEEDS_CAP = 3

export type Fold = { open: (key: string, default: boolean) -> boolean, set: (key: string, open: boolean) -> () }
export type Props = {
	of: () -> string?,
	name_of: (id: string?) -> string?,
	open_link: (link: any) -> (),
	needs: () -> number,
	fold: Fold,
	on_open_card: (chat: string, item: string?) -> (),
}

-- How many tasks it holds that aren't done: the Tasks tab's count.
function M.count(now: any, id: string?): number
	local n = 0
	for _, it in now.items or {} do
		if it.orchestrator_id == id and it.state ~= "done" and not it.removed then
			n += 1
		end
	end
	return n
end

function M.Tasks(props: Props): Node
	local function section(key: string, title: string, count: (() -> number)?, body: Node): Node
		return Fold {
			id = `orch-fold-{key}`,
			title = locale.t(title),
			count = count,
			open = function(): boolean
				return props.fold.open(key, true)
			end,
			on_toggle = function(open: boolean)
				props.fold.set(key, open)
			end,
			body,
		}
	end
	-- A card's thread opened: its detail in the panel (the card it is).
	local function open_card(chat: string)
		local id = props.of()
		for _, it in orchestration.now:peek().items or {} do
			if it.orchestrator_id == id and table.find(it.threads or {}, chat) then
				props.on_open_card(chat, it.id)
				return
			end
		end
		props.on_open_card(chat, nil)
	end
	return ui.column {
		id = "orch-tasks",
		w_full = true,
		gap = "$space.3",
		ui.div {
			w_full = true,
			hidden = function(): boolean
				return props.needs() == 0
			end,
			section("needs", "orch.panel.needs_you", props.needs, ui.div {
				id = "orch-section-needs",
				w_full = true,
				px = "$space.2",
				NeedsYou.Asks {
					now = function(): any
						return orchestration.now:get()
					end,
					sessions = function(): any
						return sessions.now:get()
					end,
					orchestrator = props.of,
					name_of = props.name_of,
					on_open = function(chat: string)
						open_card(chat)
					end,
					limit = M.NEEDS_CAP,
				},
			}),
		},
		section("board", "orch.panel.board", function(): number
			return M.count(orchestration.now:get(), props.of())
		end, stage.Later(1, function(): Node
			return ui.div {
				w_full = true,
				px = "$space.2",
				OrchBoard {
					id = "orch-page-board",
					items = function(): { any }
						return orchestration.now:get().items or {}
					end,
					orchestrators = function(): { any }
						return orchestration.now:get().orchestrators or {}
					end,
					filter = props.of,
					compact = true,
					orchestrator_of = rows.orchestrator,
					todo_of = function(chat: string): any
						local c = rows.chat(chat)
						return c and c.todo
					end,
					thread_of = function(chat: string): any
						return rows.chat(chat)
					end,
					ask_of = function(chat: string): any
						return rows.ask(chat)
					end,
					on_move = function(id: string, state: string, order: number)
						orchestration.move_item(id, state, order)
					end,
					on_pause = orchestration.pause_item,
					on_resume = orchestration.resume_item,
					asks = function(): any
						local now = orchestration.now:get()
						return now and now.asks
					end,
					on_approve = function(id: string)
						orchestration.approve_item(id)
					end,
					on_request_changes = function(id: string, text: string)
						orchestration.request_changes(id, text)
					end,
					on_priority = function(id: string, p: string)
						orchestration.set_priority(id, p)
					end,
					-- (A card opens in the panel; its detail's Open thread
					-- leaves for the thread.)
					on_open_thread = open_card,
					on_source = function(src: any)
						orchestration.reveal_dump:set(src.dump_id)
						route.control()
					end,
					on_remove = orchestration.remove_item,
					on_restore = orchestration.restore_item,
					on_start = orchestration.start_item,
					space_name = function(id: string): string?
						for _, p in sessions.projects:get() or {} do
							if p.id == id then
								return p.name
							end
						end
						return nil
					end,
					device_name = function(id: string): string?
						for _, d in sessions.devices:get() or {} do
							if d.id == id then
								return d.name
							end
						end
						return nil
					end,
					progress_of = Progress.row,
					on_link = props.open_link,
					on_answer = NeedsYou.on_answer,
					on_expand_ask = NeedsYou.on_expand,
				},
			}
		end)),
	}
end

return M

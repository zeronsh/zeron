-- An orchestrator's side panel's Tasks tab: its board as a list
-- (ds/orch_board_list): Needs you, Running, Planned and Done as folding
-- sections, a line a card, a card that needs the person carrying its ask's
-- own buttons. Where each section is folded is the panel's to keep
-- (`fold`), and a card opens in the panel's detail (`on_open_card`).
--
--   Tasks { of = fn -> orchestrator id, name_of = fn, open_link = fn,
--     needs = fn -> number, fold = { open = fn(key, default), set = fn(key, open) },
--     on_open_card = fn(chat, item?) }

local orchestration = require("@account/orchestration")
local rows = require("@account/rows")
local List = require("@ds/orch_board_list")
local stage = require("@ds/stage")
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
	-- A card opened: its detail in the panel (by its lead thread, as the
	-- panel's detail is asked for).
	local function open_card(id: string)
		for _, it in orchestration.now:peek().items or {} do
			if it.id == id then
				props.on_open_card((it.threads or {})[1] or "", it.id)
				return
			end
		end
	end
	return ui.column {
		id = "orch-tasks",
		w_full = true,
		gap = "$space.3",
		stage.Later(1, function(): Node
			return ui.div {
				w_full = true,
				px = "$space.1",
				List.List {
					id = "orch-page-board",
					items = function(): { any }
						return orchestration.now:get().items or {}
					end,
					cards = function(): { any }
						return orchestration.now:get().cards or {}
					end,
					asks = function(): any
						local now = orchestration.now:get()
						return now and now.asks
					end,
					orchestrators = function(): { any }
						return orchestration.now:get().orchestrators or {}
					end,
					filter = props.of,
					compact = true,
					fold = props.fold,
					orchestrator_of = rows.orchestrator,
					thread_of = function(chat: string): any
						return rows.chat(chat)
					end,
					progress_of = Progress.row,
					on_open_card = open_card,
					on_open_thread = function(chat: string)
						props.on_open_card(chat, nil)
					end,
					on_link = props.open_link,
					on_source = function(src: any)
						orchestration.reveal_dump:set(src.dump_id)
						route.control()
					end,
					on_answer = NeedsYou.on_answer,
					on_expand_ask = NeedsYou.on_expand,
					on_snooze = orchestration.defer,
					on_close = orchestration.close_item,
					on_reopen = orchestration.reopen_item,
					on_hold = orchestration.hold_item,
					on_pause = orchestration.pause_item,
					on_resume = orchestration.resume_item,
					on_start = function(id: string): any
						return orchestration.start_item(id)
					end,
					on_request_changes = orchestration.request_changes,
					on_remove = orchestration.remove_item,
					on_restore = orchestration.restore_item,
					on_seen = orchestration.seen_item,
				},
			}
		end),
	}
end

return M

-- An orchestrator's side panel, beside its conversation: two tabs, Tasks
-- (screens/orchestrator/tasks) and Crew (ds/orch_crew_list), each with its
-- count; a card or a crew thread opened shows its detail here in their
-- place (screens/orchestrator/detail), with a way back. Which tab shows
-- and which sections are folded are kept for each orchestrator
-- (screens/orchestrator/panel_state).
--
--   Panel { of = fn -> id, crew = fn -> members, working = fn -> number,
--     needs = fn -> number, name_of = fn, device_name = fn, open_link = fn,
--     place = State<number> }   -- where the Tasks tab is scrolled

local ds = require("@ds")
local KeptScroll = require("@ds/kept_scroll")
local CrewList = require("@ds/orch_crew_list")
local orchestration = require("@account/orchestration")
local NeedsYou = require("@app/screens/control/needs")
local Tasks = require("@app/screens/orchestrator/tasks")
local Detail = require("@app/screens/orchestrator/detail")
local state = require("@app/screens/orchestrator/panel_state")

local M = {}

M.TABS = { "tasks", "crew" }

export type Props = {
	of: () -> string?,
	crew: () -> { any },
	needs: () -> number,
	name_of: (id: string?) -> string?,
	device_name: (id: string) -> string?,
	open_link: (link: any) -> (),
	place: State<number>,
}

-- (A component: each orchestrator's tab, folds and scroll its own.)
local Body = ui.component(function(p: { props: Props, orch: string }): Node
	local props, id = p.props, p.orch
	-- (Its own, by name: never another orchestrator's.)
	local tab = ui.state(`orch_panel_tab:{id}`, state.peek(id).tab or "tasks")
	ui.effect(function()
		state.set(id, "tab", tab:get())
	end)
	local fold = {
		open = function(key: string, default: boolean): boolean
			return not state.folded(id, key, not default)
		end,
		set = function(key: string, open: boolean)
			state.fold(id, key, not open)
		end,
	}
	local function open_detail(chat: string, item: string?)
		state.detail:set({ chat = chat, item = item })
	end
	local kept = KeptScroll.use(props.place)
	-- The Crew tab's view and what's in it, measured: the room under its
	-- list, which Working fills before it folds (ds/orch_crew_list).
	local crew_view = ui.use_state(0)
	local crew_content = ui.use_state(0)
	local function measure(into: State<number>): (size: any, el: any) -> ()
		return function(size: any, _el: any)
			if math.abs(size.h - into:peek()) >= 1 then
				into:set(size.h)
			end
		end
	end
	local function scroller(sid: string, child: Node, keep: boolean, measured: boolean?): Node
		return ui.scroll {
			id = sid,
			w_full = true,
			grow = 1,
			min_h = 0,
			fade = { 12, 12 },
			fade_overflow = true,
			offset = if keep then kept.offset else nil,
			on_wheel = if keep then kept.on_wheel else nil,
			on_press = if keep then kept.on_press else nil,
			on_key = if keep then kept.on_key else nil,
			ui.column {
				w_full = true,
				on_resize = if keep then kept.on_resize elseif measured then measure(crew_content) else nil,
				px = "$space.2",
				pt = "$space.1",
				pb = "$space.6",
				child,
			},
		}
	end
	local tabs = ui.row {
		id = "orch-panel-tabs",
		w_full = true,
		shrink = 0,
		px = "$space.3",
		pt = "$space.3",
		pb = "$space.2",
		items = "center",
		ds.Tabs {
			value = tab,
			look = "pill",
			size = "sm",
			label = ui.t("orch.panel.label") :: any,
			options = {
				{
					value = "tasks",
					label = ui.t("orch.panel.tasks") :: any,
					icon = "checklist",
					count = function(): number
						return Tasks.count(orchestration.now:get(), id)
					end,
				},
				{
					value = "crew",
					label = ui.t("orch.panel.crew") :: any,
					icon = "users-group-rounded",
					count = function(): number
						return #props.crew()
					end,
				},
			},
		},
	}
	local tasks_props: Tasks.Props = {
		of = props.of,
		name_of = props.name_of,
		open_link = props.open_link,
		needs = props.needs,
		fold = fold,
		on_open_card = open_detail,
	}
	local tasks = scroller("orch-live", Tasks.Tasks(tasks_props), true)
	local crew = scroller("orch-crew-scroll", CrewList.CrewList {
		members = props.crew,
		asks = function(): { any }
			return (orchestration.now:get() or {}).asks or {}
		end,
		device_name = props.device_name,
		open = function(group: string): boolean
			return fold.open(`crew_{group}`, group ~= "idle")
		end,
		on_toggle = function(group: string, open: boolean)
			fold.set(`crew_{group}`, open)
		end,
		on_open = function(chat: string)
			open_detail(chat, nil)
		end,
		room = function(): number
			local view = crew_view:get()
			return if view > 0 then view - crew_content:get() else 0
		end,
	}, false, true)
	return ui.column {
		w_full = true,
		h_full = true,
		ui.column {
			w_full = true,
			grow = 1,
			min_h = 0,
			hidden = function(): boolean
				return state.detail:get() ~= nil
			end,
			tabs,
			ui.column {
				id = "orch-panel-tasks",
				w_full = true,
				grow = 1,
				min_h = 0,
				hidden = function(): boolean
					return tab:get() ~= "tasks"
				end,
				tasks,
			},
			ui.column {
				id = "orch-panel-crew",
				w_full = true,
				grow = 1,
				min_h = 0,
				on_resize = measure(crew_view),
				hidden = function(): boolean
					return tab:get() ~= "crew"
				end,
				crew,
			},
		},
		ui.show(function(): boolean
			return state.detail:get() ~= nil
		end, Detail.Detail {
			-- (A card's detail is the board's: it takes the board's props.)
			board = Tasks.board_props(tasks_props),
			of = function(): any
				return state.detail:get()
			end,
			orchestrator = props.of,
			name_of = props.name_of,
			device_name = props.device_name,
			on_back = function()
				state.detail:set(nil)
			end,
			on_open_thread = function(chat: string)
				state.detail:set(nil)
				NeedsYou.open_chat(chat)
			end,
		}),
	}
end)

function M.Panel(props: Props): Node
	-- (Made again for each orchestrator: its own tab and folds; a detail
	-- open on another's let go.)
	local last: string? = nil
	ui.effect(function()
		local id = props.of()
		if last ~= nil and last ~= id and state.detail:peek() ~= nil then
			state.detail:set(nil)
		end
		last = id
	end)
	return ui.presence(function(): string
		return props.of() or ""
	end, function(id: string): Node
		if id == "" then
			return ui.div {}
		end
		return Body { props = props, orch = id }
	end, { w_full = true, h_full = true, id = "orch-panel" })
end


return M

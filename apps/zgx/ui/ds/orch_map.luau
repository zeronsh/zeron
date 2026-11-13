-- The Map: where orchestration is at work, three ways.
--
--   Topology   each machine a band, its projects, and in them the
--              orchestrators and threads at work, each orchestrator's rail
--              in its hue running to its crew, across machines too
--              (ds/orch_map_topology)
--   Timeline   one lane a thread, grouped by machine and project, over the
--              last 15 minutes or hour: what each did when, its reports,
--              asks and failures as ticks, "now" at the end
--              (ds/orch_map_timeline)
--   Machines   a block a machine: its load, and the threads it runs, by
--              project and orchestrator (ds/orch_map_machines)
--   Graph      all of it as a web: machines, orchestrators, the Brain dump
--              and threads as nodes, what runs where, leads what, started
--              what as lines (ds/orch_map_graph)
--
-- A thread's state and activity show for 300 ms at least (ds/orch_hold):
-- one that changes sooner is held, then the latest wins.
--
--   Map {
--     world = function() return { now, chats, devices, projects?, loads, here } end,
--     comms = function() return orchestration.comms:get() end,
--     timeline = function() return orchestration.timeline:get() end,   -- optional
--     view = State<View>,          -- optional: which, kept by the caller
--     on_open_chat = function(id) end,
--     on_open_orchestrator = function(id) end,
--   }
--   Map.menu(view)   the widget menu's entries for choosing a view
--
-- `now` is orchestration.now's; `chats` ds/orch_data.chats; `devices` and
-- `projects` sessions.now's; `loads` orchestration.loads'; `here` this
-- device's id.

local locale = require("@self/locale")
local Topology = require("@self/orch_map_topology")
local Timeline = require("@self/orch_map_timeline")
local Machines = require("@self/orch_map_machines")
local Graph = require("@self/orch_map_graph")
local Layout = require("@self/orch_map_layout")

export type View = { variant: string, window: number }

export type Props = {
	world: () -> any,
	comms: () -> { any },
	timeline: (() -> { [string]: { any } })?,
	view: State<View>?,
	on_open_chat: (id: string) -> (),
	on_open_orchestrator: (id: string) -> (),
}

local M = {}

M.VARIANTS = { "topology", "timeline", "machines", "graph" }
M.WINDOWS = { 15, 60 }
M.DEFAULT = { variant = "topology", window = 15 } :: View

-- (Kept for what reads the Topology's layout through the Map.)
M.layout = Layout.layout

local function valid(v: any): View
	local variant = if type(v) == "table" and table.find(M.VARIANTS, v.variant) then v.variant else M.DEFAULT.variant
	local window = if type(v) == "table" and table.find(M.WINDOWS, v.window) then v.window else M.DEFAULT.window
	return { variant = variant, window = window }
end
M.valid = valid

function M.set(view: State<View>, change: { variant: string?, window: number? })
	local v = valid(view:peek())
	view:set(valid({ variant = change.variant or v.variant, window = change.window or v.window }))
end

-- The widget menu's entries: the three views, and the Timeline's span.
function M.menu(view: State<View>): { any }
	local out: { any } = { { heading = locale.s("ds.orch.map.view") } }
	for _, id in M.VARIANTS do
		table.insert(out, {
			label = locale.s(`ds.orch.map.variant.{id}`),
			icon = if id == "topology" then "git-branch" elseif id == "timeline" then "list" elseif id == "graph" then "global" else "widget",
			checked = function(): boolean
				return valid(view:get()).variant == id
			end,
			on_select = function()
				M.set(view, { variant = id })
			end,
		})
	end
	table.insert(out, { separator = true })
	for _, mins in M.WINDOWS do
		table.insert(out, {
			label = locale.s(`ds.orch.map.window_{mins}`),
			checked = function(): boolean
				return valid(view:get()).window == mins
			end,
			on_select = function()
				M.set(view, { variant = "timeline", window = mins })
			end,
		})
	end
	table.insert(out, { separator = true })
	return out
end

M.Map = ui.component(function(props: Props): Node
	local view = props.view or ui.use_state(M.DEFAULT)

	local function chat(id: string): any
		return (props.world().chats or {})[id]
	end
	local function timeline(): { [string]: { any } }
		return if props.timeline then props.timeline() or {} else {}
	end

	return ui.column {
		class = "orch-map",
		w_full = true,
		ui.presence(function(): string
			return valid(view:get()).variant
		end, function(variant: string): Node
			if variant == "timeline" then
				return Timeline.View {
					world = props.world,
					chat = chat,
					comms = props.comms,
					timeline = timeline,
					window = function(): number
						return valid(view:get()).window
					end,
					on_window = function(mins: number)
						M.set(view, { window = mins })
					end,
					on_open_chat = props.on_open_chat,
					on_open_orchestrator = props.on_open_orchestrator,
				}
			elseif variant == "graph" then
				return Graph.View {
					world = props.world,
					chat = chat,
					comms = props.comms,
					on_open_chat = props.on_open_chat,
					on_open_orchestrator = props.on_open_orchestrator,
				}
			elseif variant == "machines" then
				return Machines.View {
					world = props.world,
					chat = chat,
					on_open_chat = props.on_open_chat,
					on_open_orchestrator = props.on_open_orchestrator,
				}
			end
			return Topology {
				world = props.world,
				chat = chat,
				comms = props.comms,
				on_open_chat = props.on_open_chat,
				on_open_orchestrator = props.on_open_orchestrator,
			}
		end, { w_full = true }),
	}
end)

return M

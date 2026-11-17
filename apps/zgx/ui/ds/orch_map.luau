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
local Force = require("@self/orch_force")
local Layout = require("@self/orch_map_layout")

export type View = { variant: string, window: number }

export type Props = {
	world: () -> any,
	-- One chat by id, followed by itself (account/rows); else read out of
	-- the world, every thread following all of it.
	chat: ((id: string) -> any)?,
	-- A thread's name by itself (account/rows), for the views that show
	-- names per row; else they read it out of the world.
	name_of: ((id: string) -> string)?,
	comms: () -> { any },
	timeline: (() -> { [string]: { any } })?,
	view: State<View>?,
	on_open_chat: (id: string) -> (),
	on_open_orchestrator: (id: string) -> (),
	-- A secondary click's menu for an id (a chat's, an orchestrator's, a
	-- machine's as "dev:<id>"): its entries, or nil for none.
	menu: ((id: string) -> { any }?)?,
	-- Marks these chats done as read (the space's menu: Mark all done as
	-- read), with its Undo.
	mark_read: ((ids: { string }) -> ())?,
}

local M = {}

-- Ways the view showing opens its menu for an id (as a secondary click
-- there would), by where they hang: `Map.open_menu(id)` tries them.
-- (Keyed by the view's name, or its name and the thing's: only the view
-- showing is asked, not one on its way out.)
local openers: { [string]: (id: string) -> boolean } = {}
local showing = ""
local function register(key: string, open: ((id: string) -> boolean)?)
	openers[key] = open :: any
end
function M.open_menu(id: string): boolean
	for key, open in openers do
		if (key == showing or string.sub(key, 1, #showing + 1) == `{showing}:`) and open(id) then
			return true
		end
	end
	return false
end

-- ("force": the Graph as a web that settles once and holds still, the
-- Brain dump in the middle; beside the others while it's tried.)
M.VARIANTS = { "topology", "timeline", "machines", "graph", "force" }
M.WINDOWS = { 15, 60 }
M.DEFAULT = { variant = "force", window = 15 } :: View

-- How tall the Map stands at most: about the window (three quarters of
-- it, 360 to 900 points; 640 where the host gives no size). A view with
-- more (a Timeline or Topology of hundreds of lanes, the Machines of a big
-- fleet) scrolls inside it; the Graphs size themselves within it.
M.MIN_H, M.MAX_H, M.FALLBACK_H = 360, 900, 640
function M.max_h(): number
	local window = (ui :: any).window
	local state = if type(window) == "table" or type(window) == "userdata" then window.size else nil
	local ok, v = pcall(function(): any
		return if state ~= nil then state:get() else nil
	end)
	local h = if ok and type(v) == "table" and type(v.height) == "number" and v.height > 1 then v.height else nil
	return if h then math.clamp(math.floor(h * 0.75), M.MIN_H, M.MAX_H) else M.FALLBACK_H
end

-- A view in the Map's bounds: as tall as it is up to them, scrolled inside
-- them when taller. (A scroll sized by nothing around it takes a height of
-- its own: it's given its content's, up to the bound.)
local Bounded = ui.component(function(p: { child: Node }): Node
	local content = ui.use_state(0)
	return ui.scroll {
		class = "orch-map-scroll",
		w_full = true,
		shrink = 0,
		h = function(): number
			return math.max(1, math.min(content:get(), M.max_h()))
		end,
		fade = { 12, 12 },
		fade_overflow = true,
		ui.column {
			w_full = true,
			on_resize = function(size: { w: number, h: number })
				if math.abs(size.h - content:peek()) >= 1 then
					content:set(size.h)
				end
			end,
			p.child,
		},
	}
end)
local function bounded(child: Node): Node
	return Bounded { child = child }
end

-- (Kept for what reads the Topology's layout through the Map.)
M.layout = Layout.layout

local function valid(v: any): View
	-- (A view kept on the grid Graph, gone: opens on the force Graph.)
	if type(v) == "table" and v.variant == "graph2" then
		v = { variant = "force", window = v.window }
	end
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
			icon = if id == "topology" then "git-branch" elseif id == "timeline" then "list" elseif id == "graph" or id == "force" then "global" else "widget",
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
	-- How the Topology shows work on other machines (kept on this device).
	local TopoStyle = require("@self/orch_map_topo_style")
	for _, mode in TopoStyle.MODES do
		table.insert(out, {
			label = locale.s(`ds.orch.map.topo_{mode}`),
			checked = function(): boolean
				return TopoStyle.mode() == mode
			end,
			on_select = function()
				TopoStyle.style:set(mode)
				M.set(view, { variant = "topology" })
			end,
		})
	end
	table.insert(out, { separator = true })
	-- Which chats it shows (ds/orch_map_visibility, kept on this device):
	-- every view the same.
	local Visibility = require("@self/orch_map_visibility")
	table.insert(out, { heading = locale.s("ds.orch.map.show") })
	for _, mode in Visibility.MODES do
		table.insert(out, {
			label = locale.s(`ds.orch.map.show_{mode}`),
			checked = function(): boolean
				return Visibility.mode:get() == mode
			end,
			on_select = function()
				Visibility.mode:set(mode)
			end,
		})
	end
	table.insert(out, { separator = true })
	return out
end

-- (Until the new Graph takes the first's place: a view kept on the first
-- Graph, heavy on a big fleet, opens on the Topology, once. Chosen again,
-- it stays.)
M.left_graph = ui.persist("control_map_left_graph", false)

-- `world` (the page's) as a shape-stable function: see the Map's use.
local SHAPE_KEYS = { "now", "chats", "devices", "projects", "here", "show", "clock", "loaded", "read_at", "dismissed" }
local LISTS: { [string]: boolean } = { projects = true, devices = true }
local function same(k: string, a: any, b: any): boolean
	if a == b then
		return true
	end
	if not LISTS[k] or type(a) ~= "table" or type(b) ~= "table" or #a ~= #b then
		return false
	end
	for i = 1, #a do
		local x, y = a[i], b[i]
		if x ~= y and (type(x) ~= "table" or type(y) ~= "table" or x.id ~= y.id or x.name ~= y.name or x.online ~= y.online or x.platform ~= y.platform) then
			return false
		end
	end
	return true
end
function M.shaped(world_fn: () -> any): () -> any
	local raw: any = nil
	local kept: any = nil
	local n = 0
	local function loads(): any
		return world_fn().loads
	end
	local shape = ui.computed(function(): number
		local w = world_fn()
		local changed = raw == nil
		if not changed then
			for _, k in SHAPE_KEYS do
				if not same(k, w[k], raw[k]) then
					changed = true
					break
				end
			end
		end
		if changed then
			n += 1
			raw = w
			local t = table.clone(w)
			t.loads = nil
			kept = setmetatable(t, {
				__index = function(_self: any, key: any): any
					if key == "loads" then
						return loads()
					end
					return nil
				end,
			})
		end
		return n
	end)
	return function(): any
		shape:get()
		return kept
	end
end

M.Map = ui.component(function(props: Props): Node
	local view = props.view or ui.use_state(M.DEFAULT)
	if not M.left_graph:peek() then
		M.left_graph:set(true)
		if valid(view:peek()).variant == "graph" then
			M.set(view, { variant = "topology" })
		end
	end

	-- The world as the views read it: the same table until what it holds
	-- but the machines' loads changes (by identity; the projects and
	-- devices by what's in them). Its `loads` is read through to the
	-- page's at the moment: only what reads them (a machine's heading,
	-- its figure and ring) follows their tick, every few seconds; no
	-- view's layout does.
	local world = M.shaped(props.world)

	-- (A thread's chat: the page's own reader of one chat when it gives one
	-- (account/rows), else out of the world.)
	local chat = props.chat or function(id: string): any
		return (world().chats or {})[id]
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
			showing = variant
			if variant == "timeline" then
				return bounded(Timeline.View {
					world = world,
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
					menu = props.menu,
					register = register,
				})
			elseif variant == "graph" then
				return bounded(Graph.View {
					world = world,
					chat = chat,
					comms = props.comms,
					on_open_chat = props.on_open_chat,
					on_open_orchestrator = props.on_open_orchestrator,
					menu = props.menu,
					register = register,
				})
			elseif variant == "force" then
				return Force.View {
					world = world,
					chat = chat,
					comms = props.comms,
					on_open_chat = props.on_open_chat,
					on_open_orchestrator = props.on_open_orchestrator,
					menu = props.menu,
					mark_read = props.mark_read,
					register = register,
				}
			elseif variant == "machines" then
				return bounded(Machines.View {
					world = world,
					chat = chat,
					on_open_chat = props.on_open_chat,
					on_open_orchestrator = props.on_open_orchestrator,
					menu = props.menu,
					register = register,
				})
			end
			return bounded(Topology {
				world = world,
				chat = chat,
				name_of = props.name_of,
				comms = props.comms,
				on_open_chat = props.on_open_chat,
				on_open_orchestrator = props.on_open_orchestrator,
				menu = props.menu,
				register = register,
			})
		end, { w_full = true }),
	}
end)

return M

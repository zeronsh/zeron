-- The Map's Graph as a web that holds still: the Brain dump in the middle
-- (Material's Ghostish), each orchestrator a cookie (more scallops the more
-- crew it leads: ds/orch_force_draw) on a strong line to it, ringed and its
-- lines tinted in its own hue, its chats fanned round it on strong lines of
-- their own, thin weak lines between chats that talk or command each other,
-- a dashed "live" arc for a message in flight. A chat or an orchestrator is
-- filled with its machine's colour (ds/orch_force_palette: soft, never a
-- status's), its provider's mark in it, black or white; a chat's state is
-- its ring, as the sidebar draws it (working: the sidebar's own ring;
-- asking, approving, failed: the sidebar's glyph by it). A legend names the
-- machines.
--
-- Laid out by ds/orch_force_sim, like Obsidian's graph: stiff short
-- springs for strong links, soft long ones for weak, nodes pushing apart.
-- It opens where it last settled (the places kept, by node), a new node
-- beside its parent; a change wakes only what changed and its neighbours;
-- dragging a node pulls its own. The settling runs a few milliseconds a
-- frame and stops when still: nothing runs while nothing moves, nor off
-- screen. A pan or a zoom moves the drawing as it stands.
--
--   Force.View { world, chat, comms, on_open_chat, on_open_orchestrator,
--                menu?, register? }

local Icon = require("@self/icon")
local IconButton = require("@self/icon_button").IconButton
local HoverCard = require("@self/hover_card")
local Segmented = require("@self/segmented")
local status = require("@self/status")
local locale = require("@self/locale")
local motion = require("@self/motion")
local hue = require("@self/orch_hue")
local kit = require("@self/orch_map_kit")
local model = require("@self/orch_map_model")
local Cells = require("@self/orch_cells")
local sim = require("@self/orch_force_sim")
local fm = require("@self/orch_force_model")
local labels = require("@self/orch_force_labels")
local draw = require("@self/orch_force_draw")
local fpal = require("@self/orch_force_palette")

local M = {}

-- Where each node last settled, by id: { x, y } (kept on this device).
M.places = ui.persist("control_map_force_places", {} :: { [string]: { number } })
-- The pivot: "orchestrator" (orchestrators the hubs, machines the colour)
-- or "machine" (machines the hubs, orchestrators the colour). Kept; each
-- its own places and pins.
M.PIVOTS = { "orchestrator", "machine" }
M.pivot = ui.persist("control_map_force_pivot", "orchestrator")
M.places_m = ui.persist("control_map_force_places_machine", {} :: { [string]: { number } })
M.pins_m = ui.persist("control_map_force_pins_machine", {} :: { [string]: boolean })
function M.places_of(): any
	return if M.pivot:peek() == "machine" then M.places_m else M.places
end
function M.pins_of(): any
	return if M.pivot:peek() == "machine" then M.pins_m else M.pins
end
-- The nodes pinned by hand (dropped there), by id (kept on this device).
M.pins = ui.persist("control_map_force_pins", {} :: { [string]: boolean })
-- The node the pointer is on.
M.hover = ui.state("control_map_force_hover", nil :: string?)
-- The node held by the pointer (pressed on, not yet let go), or nil: the
-- drawing's mode is this alone, never whether anything moves.
M.held = ui.state("control_map_force_held", nil :: string?)
-- For tests and numbers: builds, wakes, steps, draws, the settling.
M.stats = ui.state("control_map_force_stats", {} :: any)
M.zoom_by = function(_k: number) end
M.pan_by = function(_dx: number, _dy: number) end
M.drag_to = function(_id: string, _x: number, _y: number) end
-- (For tests and numbers: a drag held, moved by dx, dy each call; let go.)
M.drag_by = function(_id: string, _dx: number, _dy: number) end
M.drag_end = function(_id: string) end
M.recentre = function() end
M.unpin_children = function(_id: string) end
M.unpin_all = function() end
M.current = nil :: any

-- The sidebar's glyph over a chat at `zoom`, in points: its working ring
-- (the ring's radius is 10 of its 24 units) round the chat exactly where its
-- status ring is (R_RING), or the glyph by it.
function M.ring_size(ring: boolean, zoom: number): number
	return if ring then draw.R_RING * 24 / 10 * zoom else 13 * zoom
end

-- What's gone for good from a world loaded whole: an id whose chat is
-- archived, or that's in none of its chats nor its orchestrators (an
-- archived orchestrator too); never one in `keep` (the web shown: a crew
-- chat not heard yet shows while it's new).
function M.gone(world: any, keep: { [string]: any }?): (id: string) -> boolean
	local chats = world.chats or {}
	local devices: { [string]: boolean } = {}
	for _, d in world.devices or {} do
		devices[d.id] = true
	end
	local leads: { [string]: boolean } = {}
	for _, o in (world.now or {}).orchestrators or {} do
		leads[o.id] = not o.archived
	end
	return function(id: string): boolean
		if keep and keep[id] then
			return false
		end
		-- (A machine's hub: gone with its machine.)
		if string.sub(id, 1, #fm.MACHINE) == fm.MACHINE then
			return not devices[string.sub(id, #fm.MACHINE + 1)]
		end
		local c = chats[id]
		if c then
			return c.archived == true or leads[id] == false
		end
		return not leads[id]
	end
end

-- The kept places and pins with what's gone dropped (`model`: the web
-- shown, kept whatever). Only for a world loaded whole (`world.loaded`).
function M.prune(world: any, model: any?): number
	local gone = M.gone(world, model and model.index)
	local dropped = 0
	for _, st in { M.places :: any, M.pins :: any, M.places_m :: any, M.pins_m :: any } do
		local cur = st:peek() or {}
		local out, n = {}, 0
		for id, v in cur do
			if gone(id) then
				n += 1
			else
				out[id] = v
			end
		end
		if n > 0 then
			st:set(out :: any)
			dropped += n
		end
	end
	return dropped
end

-- A frame's share of the settling (seconds), and how often it's drawn
-- while settling (frames).
local STEP_BUDGET = 0.006
-- While a node is held: a frame's share of the relaxation round it.
local DRAG_BUDGET = 0.0015
local DRAG_STEPS = 8
-- On letting go: what's left of the settling round it, in that frame.
local RELEASE_BUDGET = 0.004
local PULSE_MS = 900
local PLACES_MAX = 4000
-- How long the web keeps its shape on data loaded whole before what's gone
-- from it is swept from the places and pins (ms).
local PRUNE_MS = 3000
-- A frame's share of placing the threads' names (seconds).
local NAME_SLICE = 0.003
-- How long a zoom rests before the names are placed again at its scale (ms).
local ZOOM_SETTLE_MS = 250
-- A machine's hub on the machines pivot: Material's Clover (four leaves),
-- never a cookie (an orchestrator's).
local MACHINE_SHAPE = "clover4"
-- What a secondary click on the space (no node) is, for its menu.
local BACKGROUND = "force:background"
local TILE_H = kit.TILE_H
-- Names show from this zoom up (the important ones; others on hover).
local NAMES_ZOOM = 0.3
local R_BRAIN, R_HUB, R_CHAT, R_RING = draw.BRAIN_D / 2, draw.HUB_D / 2, draw.R_CHAT, draw.R_RING
-- The fit's least zoom: past it the web is spaced closer, never drawn
-- smaller.
local MIN_FIT = 0.75
-- A chat asking for the person; one the sidebar draws its working ring
-- for (waiting on its own work too); one it draws a glyph for, by it.
local NEED: { [string]: boolean } = { input = true }
local RINGED: { [string]: boolean } = { working = true, waiting = true }
local GLYPHED: { [string]: boolean } = { input = true, approval = true, failed = true }
-- (Where a chat's glyph sits: up and right of its middle.)
local BADGE_AT = 10.5

local T_NAME: any = { size = 11.5, baseline = "top", align = "center" }
local T_HUB: any = { size = 12.5, weight = 600, baseline = "top", align = "center" }
local T_NAME_BASE, T_HUB_BASE = T_NAME.size, T_HUB.size
local T_NAME_HALO: any = table.clone(T_NAME)
local T_HUB_HALO: any = table.clone(T_HUB)
-- A name's halo: its text again in the page's colour, these offsets round
-- it (a thread's four, fine; a fixed one's eight, fuller).
local HALO_NAME = { -1, -1, 1, -1, -1, 1, 1, 1 }
local HALO_FIXED = { -1.4, 0, 1.4, 0, 0, -1.4, 0, 1.4, -1, -1, 1, -1, -1, 1, 1, 1 }
local M_NAME = { size = 11.5 }
local M_HUB = { size = 12.5, weight = 600 }
-- (The same, in world units at the names' scale: on screen, always the
-- sizes above.)
local M_NAME_S: any = table.clone(M_NAME)
local M_HUB_S: any = table.clone(M_HUB)
local function name_opts(hub: boolean, k: number): any
	local o = if hub then M_HUB_S else M_NAME_S
	o.size = (if hub then M_HUB.size else M_NAME.size) * k
	return o
end
local DASH = { dash = { 4, 4 } }
local ROUND = { cap = "round" :: "round", join = "round" :: "round" }


M.View = ui.component(function(props: {
	world: () -> any,
	chat: (id: string) -> any,
	comms: () -> { any },
	on_open_chat: (id: string) -> (),
	on_open_orchestrator: (id: string) -> (),
	menu: ((id: string) -> { any }?)?,
	register: ((key: string, open: ((id: string) -> boolean)?) -> ())?,
}): Node
	local width = ui.use_state(0)
	local function field_h(): number
		return math.clamp(math.floor(width:get() * 0.62), 420, 760)
	end
	local view = ui.use_state({ x = 0, y = 0, zoom = 1 })
	-- (Bumped by each zoom: the names' rescale waits for it to settle.)
	local zoom_req = ui.use_state(0)
	local moved_view = false

	-- On screen: in view, in a window that shows. Off screen nothing runs.
	local in_view = ui.use_state(false)
	local function window_shown(): boolean
		local ok, shown = pcall(function()
			return ui.window.shown:get()
		end)
		return not ok or shown ~= false
	end
	local function window_shown_now(): boolean
		local ok, shown = pcall(function()
			return ui.window.shown:peek()
		end)
		return not ok or shown ~= false
	end
	local function on_screen(): boolean
		return in_view:get() and window_shown()
	end
	local function on_screen_now(): boolean
		return in_view:peek() and window_shown_now()
	end

	-- (Counts for tests and numbers, published once a frame.)
	local counts: any = { rescales = 0, leaders = 0, name_size = 0, label_halos = 0, names = 0, names_ms = 0, prunes = 0, pruned = 0, pins = 0, status = 0, rings = 0, builds = 0, wakes = 0, steps = 0, draws = 0, overlay = 0, frames = 0, settled = 0, settle_ms = 0, first_ms = 0, nodes = 0, drawn = 0, worst_ms = 0 }
	local dirty = ui.use_state(false)
	local function count(key: string, by: number?)
		counts[key] += by or 1
		if not dirty:peek() then
			dirty:set(true)
		end
	end
	ui.effect(function()
		if dirty:get() then
			ui.after(0, function()
				M.stats:set(table.clone(counts))
				dirty:set(false)
			end)
		end
	end)

	-- What's shown: the model, its sim; `version` bumps when they change
	-- (a new model, or one settled: the markers follow it then, never a
	-- frame of the settling).
	local fmodel: fm.Model = fm.build({}, {}, 0)
	local s: sim.Sim = sim.new({}, {}, 7)
	local version = ui.use_state(0)
	-- Drawn now (the settling draws every few frames).
	local tick = ui.use_state(0)
	local settled = ui.use_state(true)
	-- Where things rest: bumped when the web settles (its shapes, names and
	-- markers made again for where they now are).
	local geo = ui.use_state(0)
	-- The overlay's frames (what moves, what's lit, the dots), and the
	-- working rings' steps.
	local ptick = ui.use_state(0)
	local status_tick = ui.use_state(0)
	local run = ui.use_state(0)
	local woke_at = 0
	local holding = false
	-- (A node dragged since the web was last laid out: the view isn't fitted
	-- again for its settling.)
	local dragged_since = false
	local first_pending = false
	local open_t0 = os.clock()

	-- The chats as of the last world, for states (never a world built
	-- while drawing).
	local chats_now: { [string]: any } = {}
	local function state_of(id: string): string
		return model.status_of(chats_now[id])
	end

	-- Fitting the view to the web: its box in the field, at most 1.4x.
	-- (The names too, once placed: none cut by the field's edge.)
	local fit_names: { [number]: labels.Label } = {}
	local function fit()
		local bx0, by0, bx1, by1 = sim.bounds(s)
		local x0: number, y0: number, x1: number, y1: number = bx0, by0, bx1, by1
		local any_name = false
		for _, lab in fit_names do
			any_name = true
			local l: labels.Label = lab
			x0, y0, x1, y1 = math.min(x0, l.x0), math.min(y0, l.y0), math.max(x1, l.x1), math.max(y1, l.y1)
		end
		local w, h = math.max(1, width:peek()), field_h()
		local pad = if any_name then 24 else 70
		local z = math.clamp(math.min((w - pad * 2) / math.max(1, x1 - x0), (h - pad * 2 - 30) / math.max(1, y1 - y0)), MIN_FIT, 1.4)
		view:set({ x = w / 2 - (x0 + x1) / 2 * z, y = (h - 30) / 2 - (y0 + y1) / 2 * z, zoom = z })
		zoom_req:set(zoom_req:peek() + 1)
		counts.zoom = z
		count("frames", 0)
	end

	-- Settled: kept (by node), drawn, the markers to their places.
	local function done_settling()
		counts.settled += 1
		counts.settle_ms = (os.clock() - woke_at) * 1000
		-- (Kept for every node seen, so a node shown again, or another
		-- world's, opens where it was; at most PLACES_MAX, the web's own
		-- first.)
		local kept = table.clone(M.places_of():peek() or {})
		local n_kept = 0
		for _ in kept do
			n_kept += 1
		end
		if n_kept > PLACES_MAX then
			kept = {}
		end
		for id, at in sim.positions(s) do
			kept[id] = at
		end
		M.places_of():set(kept :: any)
		-- (Fitted after a web's own settling, never after a drag: what's
		-- shown stays where it is.)
		if not moved_view and not dragged_since then
			fit()
		end
		settled:set(true)
		geo:set(geo:peek() + 1)
		tick:set(tick:peek() + 1)
		-- (The overlay drawn once more, without what moved: its last frame of
		-- the settling would stay over the settled drawing.)
		ptick:set(ptick:peek() + 1)
		count("frames", 0)
	end

	-- The settling: a few milliseconds a frame, drawn every few frames;
	-- stopped off screen (taken up again when back).
	ui.effect(function()
		if run:get() == 0 or not on_screen() then
			return
		end
		if sim.still(s) then
			return
		end
		local frames = 0
		ui.every(16, function(): boolean?
			-- (Settled already, by a let go: nothing left.)
			if not on_screen_now() or settled:peek() then
				return false
			end
			local f0 = os.clock()
			local steps0 = s.steps
			-- (A node held by the pointer: a short relaxation a frame round
			-- it, and never at rest while it's held.)
			local still
			if holding then
				local t0 = os.clock()
				local n = 0
				repeat
					sim.step(s)
					n += 1
				until n >= DRAG_STEPS or os.clock() - t0 >= DRAG_BUDGET
				still = false
			else
				still = sim.run(s, STEP_BUDGET, os.clock)
			end
			count("steps", s.steps - steps0)
			frames += 1
			count("frames")
			counts.worst_ms = math.max(counts.worst_ms, (os.clock() - f0) * 1000)
			-- (A long settling, past MAX_STEPS since waking: held where it is.)
			if not still and not holding and s.steps - counts.wake_steps > sim.MAX_STEPS then
				sim.sleep(s)
				still = true
			end
			if still then
				done_settling()
				return false
			end
			-- (What moves is the overlay's: drawn each frame, alone; the
			-- rest stands drawn.)
			ptick:set(ptick:peek() + 1)
			return nil
		end)
	end)
	counts.wake_steps = 0
	local function wake_down(i: number)
		sim.wake_down(s, i)
		counts.wake_steps = s.steps
		woke_at = os.clock()
		count("wakes")
		settled:set(false)
		tick:set(tick:peek() + 1)
		run:set(run:peek() + 1)
	end
	local function wake(ids: { number }?)
		sim.wake(s, ids)
		counts.wake_steps = s.steps
		woke_at = os.clock()
		count("wakes")
		settled:set(false)
		-- (The rest drawn once without what's awake: that's the overlay's
		-- while it moves.)
		tick:set(tick:peek() + 1)
		run:set(run:peek() + 1)
	end

	-- A new model when what's shown changes: the same nodes and links, its
	-- states and names taken in place; else a new web, from where its
	-- nodes were (this one, else kept), waking only what's new and round it.
	-- The sweep of places and pins (M.prune), a while after the web last
	-- changed shape on data loaded whole: a world that's replaced at once
	-- (opening on a sample, a reconnect's first answer) never sweeps.
	local prune_req = ui.use_state(0)
	local prune_world: any, prune_model: any = nil, nil
	ui.effect(function()
		if prune_req:get() == 0 then
			return
		end
		ui.after(PRUNE_MS, function()
			if prune_world and prune_world.loaded == true then
				count("prunes")
				counts.pruned += M.prune(prune_world, prune_model)
			end
		end)
	end)
	local built_pivot: string? = nil
	local function rebuild(world: any, comms: { any })
		local pivot = M.pivot:peek()
		local next_m = fm.build(world, comms, world.clock or os.time(), pivot)
		if next_m.sig == fmodel.sig then
			-- The same web: what changed taken in place. A state alone (a
			-- live push): the status layer drawn again, nothing else; a
			-- name, a fade, a colour, a message in flight: the rest too.
			local marks, colours = #next_m.arcs ~= #fmodel.arcs, false
			fmodel.arcs = next_m.arcs
			for i, n in next_m.nodes do
				local o = fmodel.nodes[i]
				if o.title ~= n.title or o.fade ~= n.fade or o.provider ~= n.provider then
					marks = true
				end
				if o.hue ~= n.hue or o.device ~= n.device then
					colours = true
				end
				o.title, o.fade, o.done, o.total, o.hue, o.device, o.provider = n.title, n.fade, n.done, n.total, n.hue, n.device, n.provider
			end
			fmodel.machines = next_m.machines
			if colours or marks then
				version:set(version:peek() + 1)
				tick:set(tick:peek() + 1)
			end
			status_tick:set(status_tick:peek() + 1)
			return
		end
		count("builds")
		local t0 = os.clock()
		local nodes: { sim.Node } = table.create(#next_m.nodes) :: any
		for i, n in next_m.nodes do
			nodes[i] = { id = n.id, parent = if n.parent > 0 then n.parent else nil, mass = if n.kind == "brain" then 4 elseif n.kind == "orchestrator" or n.kind == "machine" then 2.5 else 1, fixed = n.kind == "brain" }
		end
		local links: { sim.Link } = table.create(#next_m.links) :: any
		for i, l in next_m.links do
			links[i] = { a = l.a, b = l.b, strong = l.strong, hub = l.hub }
		end
		-- (Laid out across the field: wide for a wide one; closer for a
		-- crowded web, its nodes never drawn smaller.)
		local aspect = math.max(1, width:peek()) / field_h()
		local dense = math.clamp(1 - (#nodes - 60) / 400, 0.7, 1)
		local next_s = sim.new(nodes, links, 7, { aspect = aspect, dense = dense })
		-- (Where they were: this web, else kept from before.)
		local from: { [string]: any } = table.clone(M.places_of():peek() or {})
		-- (The web as it stands, on the same pivot: a pivot's own places
		-- are never the other's.)
		if pivot == built_pivot then
			for i = 1, s.n do
				from[s.ids[i]] = { s.x[i], s.y[i] }
			end
		end
		built_pivot = pivot
		local fresh = sim.place(next_s, from)
		-- (Pinned ones held where they were dropped; a pin of a node not
		-- shown kept for when it is, till the data says it's gone.)
		for id in M.pins_of():peek() or {} do
			local i = next_s.index[id]
			if i then
				sim.pin(next_s, i, true)
			end
		end
		-- (The data loaded whole, never a world still loading: the places
		-- and pins of what's archived or gone from it go, the web's own
		-- kept.)
		-- (Loading again: a sweep due for the world before is called off.)
		prune_world, prune_model = if world.loaded == true then world else nil, next_m
		prune_req:set(prune_req:peek() + 1)
		local old_m = fmodel
		fmodel = next_m
		s = next_s
		M.current = { sim = s, model = fmodel }
		chats_now = world.chats or {}
		counts.nodes = s.n
		-- What to wake: all, on a cold start (most new); else what's new,
		-- what lost or gained a link, and their neighbours; nothing when
		-- all were where they are.
		if fresh > s.n / 2 then
			wake(nil)
		else
			local woke: { number } = {}
			local had: { [string]: number } = {}
			for _, l in old_m.links do
				local a, b = old_m.nodes[l.a].id, old_m.nodes[l.b].id
				had[if a < b then `{a}|{b}` else `{b}|{a}`] = 1
			end
			-- (A new node wakes with its neighbours; a new weak link wakes
			-- its two ends. A new node's link to its parent wakes the node,
			-- not the parent's whole crew.)
			local touched: { [number]: boolean } = {}
			for _, l in fmodel.links do
				local a, b = fmodel.nodes[l.a].id, fmodel.nodes[l.b].id
				-- (Formed between two already in the web: a world new to it
				-- opens where its places were kept.)
				if not l.strong and old_m.index[a] and old_m.index[b] and not had[if a < b then `{a}|{b}` else `{b}|{a}`] then
					touched[l.a] = true
					touched[l.b] = true
				end
			end
			-- (New to the web but with a place kept: where it rests already.)
			for i = 1, s.n do
				if s.fresh[i] then
					touched[i] = true
				end
			end
			for i in touched do
				table.insert(woke, i)
			end
			table.sort(woke)
			if #woke > 0 then
				wake(woke)
			else
				sim.sleep(s)
				settled:set(true)
				ptick:set(ptick:peek() + 1)
			end
		end
		counts.build_ms = (os.clock() - t0) * 1000
		if not moved_view and (first_pending or fresh == 0) then
			fit()
		end
		version:set(version:peek() + 1)
		tick:set(tick:peek() + 1)
	end

	ui.effect(function()
		if not on_screen() then
			return
		end
		local world = props.world()
		local comms = props.comms() or {}
		chats_now = world.chats or {}
		width:get()
		M.pivot:get()
		rebuild(world, comms)
	end)

	-- The palette, once per theme and model: no string made while drawing.
	local pal: any = nil
	local pal_theme: any, pal_ver = nil, -1
	-- Each node's fill (its machine's colour, faded with it), its mark's
	-- ink on it, the strong line to it (its orchestrator's hue), an
	-- orchestrator's outline (its hue).
	local node_fill: { string } = {}
	local node_mark: { string } = {}
	local node_line: { string } = {}
	local node_edge: { [number]: string } = {}
	local function is_dark(): boolean
		local ink = kit.ink("text.primary")
		local r, g, b = tonumber(string.sub(ink, 2, 3), 16) or 0, tonumber(string.sub(ink, 4, 5), 16) or 0, tonumber(string.sub(ink, 6, 7), 16) or 0
		return r + g + b > 384
	end
	-- A machine's colour, by its place among the machines (the same each
	-- time for the same machines).
	local function machine_fills(): { [string]: string }
		local fills = fpal.machines(is_dark())
		local out: { [string]: string } = {}
		for k, m in fmodel.machines do
			out[m.id] = fills[(k - 1) % #fills + 1]
		end
		return out
	end
	local function alpha(hex: string, a: number): string
		return string.sub(hex, 1, 7) .. string.format("%02x", math.clamp(math.floor(a * 255 + 0.5), 0, 255))
	end
	-- An orchestrator's hue as a soft fill (the machines' tone), by theme.
	local soft_made: { [string]: string } = {}
	local function soft_hue(h: number): string
		local key = `{ui.theme.current:peek()}/{h}`
		local kept = soft_made[key]
		if not kept then
			local _, _, deg = fpal.hue_of(string.sub(hue.canvas(h, 1), 1, 7))
			local tone = if is_dark() then fpal.TONE.dark else fpal.TONE.light
			kept = fpal.oklch(tone.l, tone.c, deg)
			soft_made[key] = kept
		end
		return kept
	end
	local function palette(): any
		local theme = ui.theme.current:get()
		local ver = version:get()
		if pal and pal_theme == theme and pal_ver == ver then
			return pal
		end
		pal_theme, pal_ver = theme, ver
		local dark = is_dark()
		pal = {
			dark = dark,
			fg = kit.ink("text.primary", 0.92),
			secondary = kit.ink("text.primary", 0.6),
			bg = kit.ink("background"),
			brain = kit.ink("text.primary", if dark then 0.88 else 0.82),
			-- (A node on no machine known: a plain fill.)
			body = kit.ink("text.primary", if dark then 0.2 else 0.14),
			idle = kit.ink("text.primary", 0.25),
			weak = kit.ink("text.primary", 0.14),
			live = kit.ink("text.primary", 0.78),
			ask = kit.ink("zeron.status.input"),
			approval = kit.ink("zeron.status.approval"),
			done = kit.ink("zeron.status.done"),
			failed = kit.ink("zeron.status.failed"),
			-- (A name's halo: the page's colour, a touch see-through on the
			-- frosted glass.)
			halo = kit.ink("background", 0.85),
			-- (A name's pointer to its mark: fine, faint, never a link.)
			leader = kit.ink("text.primary", 0.35),
			lit = kit.ink("text.primary", 0.85),
			-- (A fading chat's ring, by its strength in tenths.)
			fade = {},
		}
		for k = 1, 10 do
			pal.fade[k] = kit.ink("text.primary", 0.25 * k / 10)
		end
		table.clear(node_fill)
		table.clear(node_mark)
		table.clear(node_line)
		table.clear(node_edge)
		local fills = machine_fills()
		local by_machine = M.pivot:peek() ~= "machine"
		for i, n in fmodel.nodes do
			local a = if n.kind == "chat" then n.fade / 10 else 1
			-- (Its machine's colour; on the machines pivot, a thread's its
			-- orchestrator's hue, soft, and a machine hub its own colour.)
			local f = if by_machine or n.kind == "machine" then fills[n.device or ""] else (if n.hue then soft_hue(n.hue) else nil)
			if f then
				node_fill[i] = alpha(f, a)
				node_mark[i] = alpha(fpal.mark_ink(f), a)
			else
				node_fill[i] = pal.body
				node_mark[i] = alpha(pal.fg, a)
			end
			node_line[i] = if n.hue then hue.canvas(n.hue, if n.kind == "orchestrator" then 0.55 else 0.5) elseif n.kind == "machine" and f then alpha(f, 0.6) else pal.weak
			if n.kind == "orchestrator" then
				node_edge[i] = if n.hue then hue.canvas(n.hue, 1) else pal.secondary
			elseif n.kind == "machine" then
				node_edge[i] = pal.secondary
			end
		end
		return pal
	end

	-- The settled web's drawing, made once when it rests (its shapes, its
	-- marks, its names and where they go): never a frame of the settling,
	-- which draws plain discs.
	local shapes: { [number]: any } = {}
	local marks: { [number]: { { any } } } = {}
	local hub_shape: { [number]: string } = {}
	local rings: { [number]: any } = {}
	-- (Each node's name, where it goes: ds/orch_force_labels.)
	local labs: { [number]: labels.Label } = {}
	local labs_by_id: { [string]: labels.Label } = {}
	local made_for = -1
	-- (The model the shapes were made for: still right for what isn't
	-- moving while something is.)
	local made_version = -1
	local function crew_of(i: number): number
		local crew = 0
		for _, j in s.nbrs[i] do
			if s.parent[j] == i then
				crew += 1
			end
		end
		return crew
	end
	-- Every name, placed (ds/orch_force_labels): the Brain's and the
	-- orchestrators' fixed under their marks, the threads' round theirs, the
	-- ones whose node didn't move where they were while still free.
	local NAME_RANK: { [string]: number } = { input = 0, approval = 0, failed = 1, working = 2, waiting = 2, done = 3 }
	local names_job: any = nil
	-- The scale names are placed and drawn at (world units an on-screen
	-- point), and whether the zoom has moved off it since.
	local names_k = 1
	local names_dirty = false
	-- (A zoom, settled ZOOM_SETTLE_MS: the names placed again at its scale;
	-- never a wheel step's frame.)
	ui.effect(function()
		if zoom_req:get() == 0 then
			return
		end
		ui.after(ZOOM_SETTLE_MS, function()
			local k = 1 / math.max(0.05, view:peek().zoom)
			if math.abs(k - names_k) / names_k > 0.02 then
				names_dirty = true
				count("rescales")
				tick:set(tick:peek() + 1)
			end
		end)
	end)
	local names_ms = 0
	-- (Each text's width, kept: measured once.)
	local name_widths: { [string]: number } = {}
	-- A slice of the threads' names (NAME_SLICE seconds); another frame for
	-- the rest, till all are placed.
	-- (Defined with the view, below.)
	local recull: () -> () = function() end
	local names_tick = -1
	local function names_ran_this_tick(): boolean
		return names_tick == tick:peek()
	end
	local function run_names(c: any)
		local job = names_job
		if not job then
			return
		end
		names_tick = tick:peek()
		local t0 = os.clock()
		local done = labels.run(job, NAME_SLICE, function(t: string, hub: boolean): number
			return (c:measure_text(t, name_opts(hub, names_k)))
		end)
		names_ms += (os.clock() - t0) * 1000
		if not done then
			ui.after(0, function()
				if names_job == job then
					tick:set(tick:peek() + 1)
				end
			end)
			return
		end
		names_job = nil
		counts.names_ms = names_ms
		count("names")
		fit_names = job.out
		-- (Fitted again round the names, as after its own settling: never
		-- once the view's been moved or a node dragged.)
		if not moved_view and not dragged_since then
			fit()
			recull()
		end
		table.clear(labs_by_id)
		local list = {}
		for i, lab in job.out do
			labs_by_id[fmodel.nodes[i].id] = lab
			table.insert(list, { id = fmodel.nodes[i].id, kind = fmodel.nodes[i].kind, x0 = lab.x0, y0 = lab.y0, x1 = lab.x1, y1 = lab.y1, lines = lab.lines })
		end
		if M.current then
			M.current.labels = list
		end
	end
	local function place_names(c: any)
		local nodes = {}
		for i, n in fmodel.nodes do
			local p = n.parent
			table.insert(nodes, {
				id = n.id,
				kind = n.kind,
				x = s.x[i],
				y = s.y[i],
				r = if n.kind == "brain" then R_BRAIN elseif n.kind == "chat" then R_RING else R_HUB,
				title = if n.kind == "brain" then locale.s("ds.orch.map.brain") else (n.title or ""),
				hx = if p > 0 then s.x[p] else nil,
				hy = if p > 0 then s.y[p] else nil,
				rank = if n.kind == "chat" then (NAME_RANK[state_of(n.id)] or 4) else 0,
			})
		end
		local links = {}
		for _, l in fmodel.links do
			table.insert(links, { a = l.a, b = l.b, ax = s.x[l.a], ay = s.y[l.a], bx = s.x[l.b], by = s.y[l.b] })
		end
		local ts = os.clock()
		-- (At this zoom: names keep their size on screen. A new scale: every
		-- name placed afresh, its widths measured again.)
		local k = 1 / math.max(0.05, view:peek().zoom)
		if math.abs(k - names_k) > 1e-6 then
			table.clear(name_widths)
			table.clear(labs_by_id)
			names_k = k
		end
		names_dirty = false
		names_job = labels.start({
			scale = names_k,
			nodes = nodes,
			links = links,
			prev = labs_by_id,
			widths = name_widths,
			measure = function(t: string, hub: boolean): number
				return (c:measure_text(t, name_opts(hub, names_k)))
			end,
		})
		names_ms = 0
		counts.names_start_ms = (os.clock() - ts) * 1000
		-- (The fixed ones at once; the threads' a slice a frame, from here.)
		labs = names_job.out
		run_names(c)
	end
	local function make(c: any)
		local key = version:peek() * 100000 + geo:peek()
		if made_for == key or not settled:peek() then
			return
		end
		made_for = key
		made_version = version:peek()
		local tm = os.clock()
		table.clear(shapes)
		table.clear(marks)
		table.clear(hub_shape)
		table.clear(rings)
		for i, n in fmodel.nodes do
			local x, y = s.x[i], s.y[i]
			if n.kind == "brain" then
				shapes[i] = draw.shape("ghostish", x, y, draw.BRAIN_D)
			elseif n.kind == "machine" then
				hub_shape[i] = MACHINE_SHAPE
				shapes[i] = { draw.shape(MACHINE_SHAPE, x, y, draw.HUB_D), draw.shape(MACHINE_SHAPE, x, y, draw.HUB_GAP_D), draw.shape(MACHINE_SHAPE, x, y, draw.HUB_FILL_D) }
			elseif n.kind == "orchestrator" then
				local shape = draw.cookie_of(crew_of(i))
				hub_shape[i] = shape
				shapes[i] = { draw.shape(shape, x, y, draw.HUB_D), draw.shape(shape, x, y, draw.HUB_GAP_D), draw.shape(shape, x, y, draw.HUB_FILL_D) }
				local mk = draw.mark_of(n.provider)
				if mk then
					marks[i] = draw.mark(mk, x, y, draw.mark_hub(shape))
				end
			else
				local mk = draw.mark_of(n.provider)
				if mk then
					marks[i] = draw.mark(mk, x, y, draw.MARK_CHAT)
				end
				rings[i] = ui.path():arc(x, y, R_RING, 0, 360)
			end
		end
		counts.make_ms = (os.clock() - tm) * 1000
		local tp = os.clock()
		place_names(c)
		counts.place_ms = (os.clock() - tp) * 1000
	end
	local names_shown = ui.computed(function(): boolean
		return view:get().zoom >= NAMES_ZOOM
	end)

	-- What's drawn: the view's reach (world), a field past it each way;
	-- drawn again only when the view nears its edge.
	local cull = ui.use_state({ x0 = -1e9, y0 = -1e9, x1 = 1e9, y1 = 1e9 })
	recull = function()
		local v = view:peek()
		local w, h = math.max(1, width:peek()), field_h()
		local wx0, wy0 = -v.x / v.zoom, -v.y / v.zoom
		local wx1, wy1 = (w - v.x) / v.zoom, (h - v.y) / v.zoom
		local b = cull:peek()
		if wx0 >= b.x0 and wy0 >= b.y0 and wx1 <= b.x1 and wy1 <= b.y1 and (b.x1 - b.x0) <= 4 * (wx1 - wx0) then
			return
		end
		local dw, dh = wx1 - wx0, wy1 - wy0
		cull:set({ x0 = wx0 - dw / 2, y0 = wy0 - dh / 2, x1 = wx1 + dw / 2, y1 = wy1 + dh / 2 })
	end

	-- The drawing (world space; each band of the field a canvas of its own,
	-- under zgx's raster cap, moved by the view).
	local function label(c: any, i: number, t: any, ink: string, p: any)
		local lab = labs[i]
		local k = names_k
		local ld = lab.leader
		if ld then
			-- (A short pointer from the name toward its mark, a dot at the
			-- name: never a line that reads as a link.)
			c:line(ld[1], ld[2], ld[3], ld[4], p.leader, 0.7 * k, ROUND)
			c:circle(ld[1], ld[2], 1.6 * k, p.leader)
			counts.leaders += 1
		end
		-- (No box: the text alone, a halo of the page's colour round it, as
		-- the first Graph's names stand clear; heavier for the fixed names,
		-- which may sit on a line.)
		local halo: any = if t == T_HUB then T_HUB_HALO else T_NAME_HALO
		local size = (if t == T_HUB then T_HUB_BASE else T_NAME_BASE) * k
		halo.color = p.halo
		halo.align = lab.align
		halo.size = size
		t.color = ink
		t.align = lab.align
		t.size = size
		counts.name_size = T_NAME_BASE * k
		-- (For tests: names are drawn with no box behind them.)
		counts.label_boxes = 0
		local lh = lab.h / #lab.lines
		local offs = if lab.fixed then HALO_FIXED else HALO_NAME
		for li, line in lab.lines do
			local ly = lab.y + (li - 1) * lh
			for o = 1, #offs, 2 do
				c:text(line, lab.x + offs[o] * k, ly + offs[o + 1] * k, halo)
			end
			counts.label_halos += 1
			c:text(line, lab.x, ly, t)
		end
	end
	-- One line a pair: a pair with a message in flight is drawn by its arc
	-- alone (dashed, curved), never its straight weak line as well; lit, the
	-- same arc is restyled. (By the pair, either way round.)
	local function pair_of(a: number, b: number): number
		return if a < b then a * 65536 + b else b * 65536 + a
	end
	local function arc_of(a: number, b: number): any
		local key = pair_of(a, b)
		for _, arc in fmodel.arcs do
			if pair_of(arc.a, arc.b) == key then
				return arc
			end
		end
		return nil
	end
	local function arc_path(arc: any): any
		local x, y = s.x, s.y
		local ax, ay, bx, byy = x[arc.a], y[arc.a], x[arc.b], y[arc.b]
		local mx, my = (ax + bx) / 2, (ay + byy) / 2
		local dx, dy = bx - ax, byy - ay
		return ui.path():move(ax, ay):quad(mx - dy * 0.18, my + dx * 0.18, bx, byy)
	end
	local function fill_mark(c: any, i: number)
		local m = marks[i]
		if m then
			local ink = node_mark[i]
			for _, part in m do
				c:fill(part[1], ink, part[2])
			end
		end
	end
	-- (For tests: how each weak pair was drawn last, and lit.)
	local edge_style: { [number]: string } = {}
	local lit_style: { [number]: string } = {}
	local function draw_web(c: any)
		local p = palette()
		table.clear(edge_style)
		if M.current then
			M.current.edge_style, M.current.lit_style = edge_style, lit_style
		end
		local b = cull:get()
		local still = settled:peek()
		make(c)
		if names_dirty and made_version == version:peek() and settled:peek() and not names_job then
			place_names(c)
		elseif names_job and made_version == version:peek() and not names_ran_this_tick() then
			run_names(c)
		end
		local full = made_version == version:peek()
		-- (What's awake moves on the overlay: left out here.)
		local moving = not still
		-- (Each band draws all the view reaches, not only its own strip: a
		-- pan moves the drawing without drawing it again, so what comes into
		-- a band's strip must be there already. The canvas clips to its
		-- strip; the view's reach is what culls.)
		local x0, y0, x1, y1 = b.x0, b.y0, b.x1, b.y1
		local x, y = s.x, s.y
		local nodes = fmodel.nodes
		local awake = s.awake
		-- Links: weak (hairlines), then strong (in the orchestrator's hue, a
		-- hub's heavier).
		for _, l in fmodel.links do
			if moving and (awake[l.a] or awake[l.b]) then
				continue
			end
			local ax, ay, bx, byy = x[l.a], y[l.a], x[l.b], y[l.b]
			if math.max(ax, bx) < x0 or math.min(ax, bx) > x1 or math.max(ay, byy) < y0 or math.min(ay, byy) > y1 then
				continue
			end
			-- (Heavier with its traffic: a hairline, up to a point and a half;
			-- one with a message in flight is its arc's, below.)
			if not l.strong and not (full and not moving and arc_of(l.a, l.b)) then
				c:line(ax, ay, bx, byy, p.weak, 0.8 + math.min((l :: any).count or 0, 7) * 0.1)
				edge_style[pair_of(l.a, l.b)] = "line"
			end
		end
		for _, l in fmodel.links do
			if l.strong and not (moving and (awake[l.a] or awake[l.b])) then
				local ax, ay, bx, byy = x[l.a], y[l.a], x[l.b], y[l.b]
				if math.max(ax, bx) < x0 or math.min(ax, bx) > x1 or math.max(ay, byy) < y0 or math.min(ay, byy) > y1 then
					continue
				end
				c:line(ax, ay, bx, byy, node_line[l.b], if l.hub then 2.4 else 1.3)
			end
		end
		-- Marks: the shape, its fill, its provider's mark.
		local show_names = names_shown:get() and full
		local drawn = 0
		for i, n in nodes do
			local nx, ny = x[i], y[i]
			if (moving and awake[i]) or nx < x0 - 40 or nx > x1 + 40 or ny < y0 - 40 or ny > y1 + 40 then
				continue
			end
			drawn += 1
			if n.kind == "brain" then
				if full then
					c:fill(shapes[i], p.brain)
				else
					c:circle(nx, ny, R_BRAIN, p.brain)
				end
			elseif n.kind == "orchestrator" or n.kind == "machine" then
				-- (Its outline in its hue, following its shape; a gap; its
				-- machine's fill.)
				if full then
					local sh = shapes[i]
					c:fill(sh[1], node_edge[i])
					c:fill(sh[2], p.bg)
					c:fill(sh[3], node_fill[i])
					fill_mark(c, i)
				else
					c:circle(nx, ny, R_HUB, node_edge[i])
					c:circle(nx, ny, draw.HUB_GAP_D / 2, p.bg)
					c:circle(nx, ny, draw.HUB_FILL_D / 2, node_fill[i])
				end
			else
				-- (Its state's ring, glyph and name are the status layer's:
				-- a state changing draws that alone.)
				c:circle(nx, ny, R_CHAT, node_fill[i])
				if full then
					fill_mark(c, i)
				end
			end
		end
		-- Names after every mark, their ground over what's under them (in a
		-- crowd, a name sits over chats).
		if show_names then
			for i, n in nodes do
				local lab = labs[i]
				if lab and lab.x1 >= x0 and lab.x0 <= x1 and lab.y1 >= y0 and lab.y0 <= y1 and not (moving and awake[i]) then
					label(c, i, if n.kind == "chat" then T_NAME else T_HUB, if n.kind == "chat" then p.secondary else p.fg, p)
				end
			end
		end
		-- Live: a dashed arc for each message in flight.
		if full and not moving then
			for _, a in fmodel.arcs do
				c:stroke(arc_path(a), p.live, 1.4, DASH)
				edge_style[pair_of(a.a, a.b)] = "arc"
			end
		end
		return drawn
	end

	-- The overlay: what moves (plain shapes, a frame at a time), what's lit
	-- (the node under the pointer: its shape's outline, its links; pinned, a
	-- pin by it), the dots running lines.
	local pulses: { any } = {}
	local function draw_over(c: any)
		local p = palette()
		local x, y = s.x, s.y
		-- (For tests: how many moving it drew last, none once settled.)
		local moving_drawn = 0
		if not settled:peek() then
			local awake = s.awake
			for _, l in fmodel.links do
				if awake[l.a] or awake[l.b] then
					c:line(x[l.a], y[l.a], x[l.b], y[l.b], if l.strong then node_line[l.b] else p.weak, if l.hub then 2.4 elseif l.strong then 1.3 else 0.8)
				end
			end
			for i, n in fmodel.nodes do
				if awake[i] then
					moving_drawn += 1
					if n.kind == "brain" then
						c:circle(x[i], y[i], R_BRAIN, p.brain)
					elseif n.kind == "orchestrator" or n.kind == "machine" then
						c:circle(x[i], y[i], R_HUB, node_edge[i])
						c:circle(x[i], y[i], draw.HUB_GAP_D / 2, p.bg)
						c:circle(x[i], y[i], draw.HUB_FILL_D / 2, node_fill[i])
					else
						local st = state_of(n.id)
						local ring = if NEED[st] then p.ask elseif st == "approval" then p.approval elseif st == "done" then p.done elseif st == "failed" then p.failed else p.idle
						c:circle(x[i], y[i], R_RING + 0.8, ring)
						c:circle(x[i], y[i], R_RING - 0.8, p.bg)
						c:circle(x[i], y[i], R_CHAT, node_fill[i])
					end
				end
			end
		end
		if counts.over_moving ~= moving_drawn then
			counts.over_moving = moving_drawn
			count("overlay", 0)
		end
		table.clear(lit_style)
		local over = M.hover:get()
		local i = over and fmodel.index[over]
		if i then
			local n = fmodel.nodes[i]
			for _, l in fmodel.links do
				if l.a == i or l.b == i then
					-- (Lit as it's drawn: a pair's arc restyled, not a second,
					-- straight line.)
					local arc = if l.strong or not settled:peek() then nil else arc_of(l.a, l.b)
					if arc then
						c:stroke(arc_path(arc), p.lit, 1.6, DASH)
						lit_style[pair_of(l.a, l.b)] = "arc"
					else
						c:line(x[l.a], y[l.a], x[l.b], y[l.b], p.lit, if l.strong then 1.8 else 1)
						lit_style[pair_of(l.a, l.b)] = "line"
					end
				end
			end
			local nx, ny = x[i], y[i]
			local r
			if n.kind == "brain" then
				c:stroke(draw.shape("ghostish", nx, ny, draw.BRAIN_D + 9), p.lit, 1.6)
				r = R_BRAIN
			elseif n.kind == "orchestrator" or n.kind == "machine" then
				c:stroke(draw.shape(hub_shape[i] or draw.cookie_of(crew_of(i)), nx, ny, draw.HUB_D + 8), p.lit, 1.6)
				r = R_HUB
			else
				c:stroke(ui.path():arc(nx, ny, R_RING + 3.5, 0, 360), p.lit, 1.6)
				r = R_RING
			end
			-- (Pinned by hand: a pin at its top left, while it's lit.)
			if s.pinned[i] and not s.fixed[i] then
				local d = r * 0.72 + 10
				local pin, w = draw.pin(nx - d, ny - d, 16)
				c:circle(nx - d, ny - d, 10, p.bg)
				c:stroke(pin, p.fg, w, ROUND)
				count("pins")
			end
		end
		local now = ui.now()
		for _, q in pulses do
			local t = (now - q.t0) / PULSE_MS
			if t >= 0 and t <= 1 then
				local a, b = q.a, q.b
				local e = 1 - (1 - t) ^ 3
				c:circle(x[a] + (x[b] - x[a]) * e, y[a] + (y[b] - y[a]) * e, 3, p.live)
			end
		end
	end
	-- The status layer: each chat's ring for its state (working's is the
	-- sidebar's own ring, over the canvas), the ground under the sidebar's
	-- glyph for one asking, approving or failed, and the names of those
	-- asking, placed free of the rest. Drawn again for a state's change
	-- alone (a live push), nothing else with it.
	local function draw_status(c: any)
		local p = palette()
		if made_version ~= version:peek() then
			return
		end
		local still = settled:peek()
		local x, y = s.x, s.y
		local b = cull:get()
		for i, n in fmodel.nodes do
			if n.kind ~= "chat" or not rings[i] or (not still and s.awake[i]) then
				continue
			end
			local nx, ny = x[i], y[i]
			if nx < b.x0 - 40 or nx > b.x1 + 40 or ny < b.y0 - 40 or ny > b.y1 + 40 then
				continue
			end
			local st = state_of(n.id)
			if RINGED[st] then
				continue
			end
			local ink = if NEED[st] then p.ask elseif st == "approval" then p.approval elseif st == "done" then p.done elseif st == "failed" then p.failed elseif n.fade < 10 then p.fade[n.fade] else p.idle
			c:stroke(rings[i], ink, if st == "idle" then 1.1 else 1.6)
			if GLYPHED[st] then
				c:circle(nx + BADGE_AT, ny - BADGE_AT, 7.2, p.bg)
			end
		end
	end
	-- Dots: once per message, along a straight line between its ends.
	local seen: { [string]: boolean }? = nil
	local pulse_run = ui.use_state(0)
	ui.effect(function()
		local list = props.comms() or {}
		if seen == nil or not on_screen_now() then
			seen = seen or {}
			for _, e in list do
				(seen :: any)[e.id] = true
			end
			return
		end
		local fresh = false
		for _, e in list do
			if not (seen :: any)[e.id] then
				(seen :: any)[e.id] = true
				local a, b = fmodel.index[e.from or ""], fmodel.index[e.to or ""]
				if a and b and not motion.reduced_now() then
					table.insert(pulses, { a = a, b = b, t0 = ui.now() })
					fresh = true
				end
			end
		end
		if fresh then
			pulse_run:set(pulse_run:peek() + 1)
		end
	end)
	ui.effect(function()
		if pulse_run:get() == 0 then
			return
		end
		ui.every(33, function(): boolean?
			local now = ui.now()
			local k = 1
			while k <= #pulses do
				if now - pulses[k].t0 > PULSE_MS or not on_screen_now() then
					table.remove(pulses, k)
				else
					k += 1
				end
			end
			ptick:set(ptick:peek() + 1)
			return if #pulses > 0 then nil else false
		end)
	end)

	-- The view.
	local function set_view(x: number, y: number, zoom: number)
		local was = view:peek().zoom
		view:set({ x = x, y = y, zoom = zoom })
		if math.abs(zoom - was) > 1e-6 then
			zoom_req:set(zoom_req:peek() + 1)
		end
		counts.zoom = zoom
		count("frames", 0)
		recull()
	end
	local function zoom_at(px: number, py: number, k: number)
		moved_view = true
		local v = view:peek()
		local zoom = math.clamp(v.zoom * k, 0.25, 3)
		local wx, wy = (px - v.x) / v.zoom, (py - v.y) / v.zoom
		set_view(px - wx * zoom, py - wy * zoom, zoom)
	end
	M.zoom_by = function(k: number)
		zoom_at(width:peek() / 2, field_h() / 2, k)
	end
	M.pan_by = function(dx: number, dy: number)
		moved_view = true
		local v = view:peek()
		set_view(v.x + dx, v.y + dy, v.zoom)
	end
	ui.effect(function()
		width:get()
		cull:set({ x0 = -1e9, y0 = -1e9, x1 = 1e9, y1 = 1e9 })
	end)

	-- The pointer: a node under it (nearest, within its reach); a press on
	-- a node and a drag moves it (its neighbours follow), on the space pans.
	local function to_world(px: number, py: number): (number, number)
		local v = view:peek()
		return (px - v.x) / v.zoom, (py - v.y) / v.zoom
	end
	local function hit(px: number, py: number): string?
		local wx, wy = to_world(px, py)
		local z = view:peek().zoom
		local best, best_d = nil, math.huge
		for i, n in fmodel.nodes do
			local r = (if n.kind == "brain" then R_BRAIN elseif n.kind == "chat" then R_RING else R_HUB) + 4 / z
			local dx, dy = s.x[i] - wx, s.y[i] - wy
			local d = dx * dx + dy * dy
			if d <= r * r and d < best_d then
				best, best_d = n.id, d
			end
		end
		return best
	end
	local function drag(id: string, wx: number, wy: number)
		local i = fmodel.index[id]
		if not i then
			return
		end
		-- (The Brain too: it moves, its hubs pulled along; let go, it stays
		-- there, the web's anchor where it was dropped.)
		s.held[i] = true
		dragged_since = true
		if not holding then
			holding = true
			if M.held:peek() ~= fmodel.nodes[i].id then
				M.held:set(fmodel.nodes[i].id)
			end
		end
		s.x[i], s.y[i] = wx, wy
		-- (Woken once: held, it never rests, so nothing wakes it again.)
		-- (The Brain wakes the whole web: its hubs follow, their crews
		-- after them.)
		if not s.awake[i] then
			if s.fixed[i] then
				wake(nil)
			else
				wake_down(i)
			end
		end
	end
	-- Let go: it stays where it was dropped (its slot now), the few round
	-- it settle in this frame, and the drawing is still again at once.
	local function let_go(i: number, dropped: boolean?)
		s.held[i] = s.fixed[i] or s.pinned[i]
		if M.held:peek() ~= nil then
			M.held:set(nil)
		end
		if not holding then
			return
		end
		holding = false
		sim.adopt(s, i)
		-- (Dropped, moved there: pinned, its parent moving or not; a click
		-- pins nothing.)
		if dropped ~= false and not s.fixed[i] then
			sim.pin(s, i, true)
			if not (M.pins_of():peek() or {})[s.ids[i]] then
				M.pins_of():set_key({ s.ids[i] }, true)
			end
		end
		if not settled:peek() then
			local steps0 = s.steps
			sim.run(s, RELEASE_BUDGET, os.clock)
			count("steps", s.steps - steps0)
			sim.sleep(s)
			done_settling()
		end
	end
	-- Let go of pins: a parent's children's (the Brain's: its own children,
	-- orchestrators and its chats, not theirs), or all; they settle back
	-- into the web round their parents.
	local function unpin(which: (i: number) -> boolean)
		local woke: { number } = {}
		local pins = table.clone(M.pins_of():peek() or {})
		for i = 1, s.n do
			if s.pinned[i] and which(i) then
				sim.pin(s, i, false)
				pins[s.ids[i]] = nil
				table.insert(woke, i)
			end
		end
		if #woke > 0 then
			M.pins_of():set(pins :: any)
			wake(woke)
		end
	end
	M.unpin_children = function(id: string)
		local p = fmodel.index[id]
		if p then
			unpin(function(i: number): boolean
				return s.parent[i] == p
			end)
		end
	end
	M.unpin_all = function()
		unpin(function(_i: number): boolean
			return true
		end)
	end

	-- Fit: the Brain back in the centre (where it was dragged, if it was),
	-- its hubs following, the rest where they are round them.
	local function recentre()
		local bx, by = 0, 0
		for i = 1, s.n do
			if s.fixed[i] then
				bx, by = s.x[i], s.y[i]
			end
		end
		if bx == 0 and by == 0 then
			return
		end
		-- (The whole web moved with it: nothing else changes shape.)
		for i = 1, s.n do
			s.x[i] -= bx
			s.y[i] -= by
		end
		if settled:peek() then
			done_settling()
		end
	end
	M.recentre = recentre
	M.drag_by = function(id: string, dx: number, dy: number)
		local i = fmodel.index[id]
		if i then
			drag(id, s.x[i] + dx, s.y[i] + dy)
		end
	end
	M.drag_end = function(id: string)
		local i = fmodel.index[id]
		if i then
			let_go(i)
		end
	end
	M.drag_to = function(id: string, wx: number, wy: number)
		drag(id, wx, wy)
		local i = fmodel.index[id]
		if i then
			let_go(i)
		end
	end
	local press: any = nil
	local grip_hot = ui.use_state(false)
	local in_field = false
	-- Pressed on a node: held from now (the drawing's dragging mode) till
	-- let go or cancelled, moved or not.
	local function on_press(px: number, py: number)
		local id = hit(px, py)
		press = { id = id, x = px, y = py, moved = false, view = view:peek() }
		local i = id and fmodel.index[id]
		if i then
			drag(id :: string, s.x[i], s.y[i])
		end
	end
	-- (Cancelled, the window hidden or covered mid-drag: let go where it is.)
	local function cancel()
		local p = press
		press = nil
		local i = p and p.id and fmodel.index[p.id]
		if i then
			let_go(i)
		end
	end
	local function on_move(px: number, py: number)
		if press then
			local dx, dy = px - press.x, py - press.y
			if not press.moved and dx * dx + dy * dy < 16 then
				return
			end
			press.moved = true
			if press.id then
				local wx, wy = to_world(px, py)
				drag(press.id, wx, wy)
			else
				moved_view = true
				set_view(press.view.x + dx, press.view.y + dy, press.view.zoom)
			end
			return
		end
		local id = hit(px, py)
		if id ~= M.hover:peek() then
			M.hover:set(id)
		end
	end
	ui.effect(function()
		if not window_shown() and press then
			cancel()
		end
	end)
	local function open(id: string)
		local i = fmodel.index[id]
		local n = i and fmodel.nodes[i]
		if not n then
			return
		end
		if n.kind == "chat" then
			props.on_open_chat(id)
		elseif n.kind ~= "machine" then
			props.on_open_orchestrator(id)
		end
	end
	local function on_release(px: number, py: number)
		local p = press
		press = nil
		if not p then
			return
		end
		local i = p.id and fmodel.index[p.id]
		if i then
			let_go(i, p.moved)
		end
		if not p.moved then
			local id = hit(px, py)
			if id then
				open(id)
			end
		end
	end
	local function leave_later()
		ui.after(0, function()
			if not grip_hot:peek() and not in_field and not press then
				M.hover:set(nil)
			end
		end)
	end
	local function screen_of(id: string): (number, number, number)
		local i = fmodel.index[id]
		if not i then
			return -1000, -1000, 0
		end
		local v = view:peek()
		local n = fmodel.nodes[i]
		local r = if n.kind == "brain" then R_BRAIN elseif n.kind == "chat" then R_RING else R_HUB
		return s.x[i] * v.zoom + v.x, s.y[i] * v.zoom + v.y, r * v.zoom
	end

	-- The card of what the pointer rests on (ds/hover_card's anchored).
	local function card(): Node
		local function id(): string?
			return M.hover:get()
		end
		local function n(): any
			version:get()
			local x = id()
			local i = x and fmodel.index[x]
			return if i then fmodel.nodes[i] else nil
		end
		local function c(): any
			local x = id()
			return if x then props.chat(x) or {} else {}
		end
		local function st(): string
			local x = id()
			return if x then state_of(x) else "idle"
		end
		local function machine(): string
			local x = n()
			for _, m in fmodel.machines do
				if x and m.id == x.device then
					return m.name
				end
			end
			return ""
		end
		local function doing(): string
			local s0 = st()
			local a = c().activity
			if s0 == "working" and a then
				return locale.s(`ds.orch.activity.{a.kind}`)
			end
			return if s0 ~= "idle" and status.KINDS[s0 :: any] then status.KINDS[s0 :: any].long else ""
		end
		return HoverCard.Card {
			class = "orch-force-card",
			w = 248,
			p = "$space.3",
			gap = "$space.1",
			ui.row {
				gap = "$space.2",
				items = "center",
				ui.show(function(): boolean
					local x = n()
					return x ~= nil and x.kind == "chat" and st() ~= "idle"
				end, status.StatusIcon { kind = function(): any
					return st()
				end, size = 14 }),
				ui.text {
					function(): string
						local x = n()
						if x and x.kind == "brain" then
							return locale.s("ds.orch.map.brain")
						end
						return if x then x.title else ""
					end,
					grow = 1,
					min_w = 0,
					truncate = true,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					color = "$text.primary",
				},
			},
			ui.show(function(): boolean
				return doing() ~= ""
			end, ui.text { doing, text_size = "$font.size.xs", color = "$text.secondary" }),
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				ui.div { color = "$text.tertiary", shrink = 0, Icon { name = "monitor", size = 12 } },
				ui.text { machine, text_size = "$font.size.xs", color = "$text.tertiary", truncate = true, min_w = 0 },
				ui.text {
					function(): string
						return c().model or ""
					end,
					grow = 1,
					align = "end",
					truncate = true,
					min_w = 0,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			},
			ui.show(function(): boolean
				local t = c().todo
				return t ~= nil and (t.total or 0) > 0
			end, Cells {
				done = function(): number
					return (c().todo or {}).done or 0
				end,
				total = function(): number
					return (c().todo or {}).total or 0
				end,
				current = function(): boolean
					return st() == "working"
				end,
				fill = true,
				w = 224,
				h = 8,
			}),
		}
	end
	local function hover_box(): (number, number, number)
		view:get()
		ptick:get()
		geo:get()
		local id = M.hover:get()
		if not id then
			return -1000, -1000, 0
		end
		return screen_of(id)
	end
	local anchor_at = { x = 0, y = 0 }
	local anchor = HoverCard.Anchored {
		class = "orch-force-anchor",
		w = 248,
		delay = 350,
		placement = { side = "bottom", align = "start", offset = 6 },
		anchor = ui.div {
			class = "orch-force-anchor-box",
			w = function(): number
				local _, _, r = hover_box()
				return math.max(1, r * 2)
			end,
			h = function(): number
				local _, _, r = hover_box()
				return math.max(1, r * 2)
			end,
			on_press = function(e: any)
				on_press(anchor_at.x + e.local_x, anchor_at.y + e.local_y)
			end,
			on_pointer_move = function(e: any)
				on_move(anchor_at.x + e.local_x, anchor_at.y + e.local_y)
			end,
			on_release = function(e: any)
				on_release(anchor_at.x + e.local_x, anchor_at.y + e.local_y)
			end,
			on_hover = function(inside: boolean)
				in_field = inside
				if not inside and not press then
					leave_later()
				end
			end,
		},
		card = function(_c: any): Node
			return card()
		end,
	}

	-- Markers (assistive technology, tests): each node's place once
	-- settled, in a layer the pan moves whole; none off screen.
	local items = ui.computed(function(): { any }
		version:get()
		local out = {}
		for _, n in fmodel.nodes do
			table.insert(out, { id = n.id, kind = n.kind })
		end
		return out
	end)
	local function marker(item: any): Node
		local function at(): (number, number)
			version:get()
			geo:get()
			local i = fmodel.index[item.id]
			if not i then
				return -1000, -1000
			end
			local z = view:get().zoom
			return s.x[i] * z, s.y[i] * z
		end
		return ui.div {
			class = `orch-force-node orch-force-{item.kind}`,
			label = function(): string
				version:get()
				local i = fmodel.index[item.id]
				local n = i and fmodel.nodes[i]
				local t = if n then n.title else ""
				local st = state_of(item.id)
				if item.kind == "chat" and st ~= "idle" and status.KINDS[st :: any] then
					return `{t}, {status.KINDS[st :: any].long}`
				end
				return t
			end,
			absolute = true,
			pointer_events = "none",
			left = function(): number
				local x = at()
				return x - 4
			end,
			top = function(): number
				local _, y = at()
				return y - 4
			end,
			w = 8,
			h = 8,
		}
	end

	-- The sidebar's own status glyphs over the canvas (ds/status's
	-- StatusIcon): working's ring round a working chat (its animation
	-- the sidebar's, nothing when none works), asking's, approving's and
	-- failed's glyph by one; in the layer the pan moves whole. One moving
	-- (awake, dragged) is left out till it rests: the overlay draws it.
	local zoom = ui.computed(function(): number
		return view:get().zoom
	end)
	-- (Made the frame after the web's first drawing, not with it; only for
	-- the chats in the view's reach.)
	local glyphs_on = ui.use_state(false)
	local glyphed = ui.computed(function(): { any }
		version:get()
		status_tick:get()
		geo:get()
		local b = cull:get()
		local out = {}
		for i, n in fmodel.nodes do
			local x, y = s.x[i], s.y[i]
			if n.kind == "chat" and x >= b.x0 - 40 and x <= b.x1 + 40 and y >= b.y0 - 40 and y <= b.y1 + 40 then
				local st = state_of(n.id)
				if RINGED[st] or GLYPHED[st] then
					table.insert(out, { id = n.id, ring = RINGED[st] == true })
				end
			end
		end
		return out
	end)
	local ring_size = M.ring_size
	local function glyph_el(item: any): Node
		-- (The ring's radius is 10 of its 24: sized to the chat's ring.)
		-- (Followed as the zoom moves, never the size it was made at.)
		local function size(): number
			return ring_size(item.ring, zoom:get())
		end
		local function at(): (number, number)
			version:get()
			geo:get()
			local i = fmodel.index[item.id]
			if not i then
				return -1000, -1000
			end
			local z = zoom:get()
			if item.ring then
				return s.x[i] * z, s.y[i] * z
			end
			return (s.x[i] + BADGE_AT) * z, (s.y[i] - BADGE_AT) * z
		end
		return ui.div {
			class = if item.ring then "orch-force-ring" else "orch-force-glyph",
			absolute = true,
			pointer_events = "none",
			left = function(): number
				return (at()) - size() / 2
			end,
			top = function(): number
				local _, y = at()
				return y - size() / 2
			end,
			opacity = function(): number
				tick:get()
				local i = fmodel.index[item.id]
				return if i and not settled:peek() and s.awake[i] then 0 else 1
			end,
			status.StatusIcon {
				kind = function(): any
					status_tick:get()
					return state_of(item.id)
				end,
				size = size,
			},
		}
	end

	-- The machines' legend: each one's colour and name, at the bottom left.
	local legend = ui.computed(function(): { any }
		version:get()
		ui.theme.current:get()
		local fills = machine_fills()
		local used: { [string]: boolean } = {}
		for _, n in fmodel.nodes do
			if n.device then
				used[n.device] = true
			end
		end
		local out = {}
		-- (On the machines pivot: the orchestrators, by their hue.)
		if M.pivot:get() == "machine" then
			for _, n in fmodel.nodes do
				if n.lead_of and n.hue then
					table.insert(out, { id = n.id, name = n.title, color = soft_hue(n.hue) })
				end
			end
			return out
		end
		for _, m in fmodel.machines do
			if used[m.id] and fills[m.id] then
				table.insert(out, { id = m.id, name = m.name, color = fills[m.id] })
			end
		end
		return out
	end)
	local function legend_item(item: any): Node
		return ui.row {
			class = "orch-force-legend-item",
			gap = "$space.1",
			items = "center",
			ui.div { w = 8, h = 8, shrink = 0, rounded = "$radius.full", bg = item.color },
			ui.text { item.name or item.id, text_size = "$font.size.xs", color = "$text.secondary", truncate = true, max_w = 140 },
		}
	end

	-- The bands: the field in canvases TILE_H high (zgx rasterizes a canvas
	-- whole, up to 32 MiB), each moved by the view.
	local bands = ui.computed(function(): { number }
		local out = {}
		for k = 0, math.max(0, math.ceil(field_h() / TILE_H) - 1) do
			table.insert(out, k)
		end
		return out
	end)
	local function band(k: number): Node
		local top = k * TILE_H
		local function band_view(): any
			local v = view:get()
			return { x = v.x, y = v.y - top, zoom = v.zoom }
		end
		local function bottom(): number
			return math.min(top + TILE_H, field_h())
		end
		return ui.div {
			absolute = true,
			left = 0,
			right = 0,
			top = top,
			h = function(): number
				return math.max(1, bottom() - top)
			end,
			pointer_events = "none",
			ui.canvas {
				class = "orch-force-canvas",
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				pointer_events = "none",
				view = band_view,
				draw = function(c: any, _w: number, _h: number)
					if not on_screen() then
						return
					end
					tick:get()
					version:get()
					local t0 = os.clock()
					local n = draw_web(c)
					if not glyphs_on:peek() then
						ui.after(0, function()
							glyphs_on:set(true)
						end)
					end
					count("draws")
					counts.drawn = n
					-- (Each band's own count: every band draws the view's
					-- whole reach.)
					counts[`drawn_{k}`] = n
					local ms = (os.clock() - t0) * 1000
					counts.draw_ms = ms
					if first_pending then
						first_pending = false
						counts.first_ms = (os.clock() - open_t0) * 1000
					end
				end,
			},
			ui.canvas {
				class = "orch-force-status",
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				pointer_events = "none",
				view = band_view,
				draw = function(c: any, _w: number, _h: number)
					if not on_screen() then
						return
					end
					status_tick:get()
					tick:get()
					version:get()
					count("status")
					draw_status(c)
				end,
			},
			ui.canvas {
				class = "orch-force-overlay",
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				pointer_events = "none",
				view = band_view,
				draw = function(c: any, _w: number, _h: number)
					if not on_screen() then
						return
					end
					ptick:get()
					count("overlay")
					draw_over(c)
				end,
			},
		}
	end

	return ui.column {
		class = "orch-map-force",
		w_full = true,
		relative = true,
		gap = "$space.2",
		on_resize = function(size: { w: number, h: number })
			if math.abs(size.w - width:peek()) >= 1 then
				width:set(size.w)
			end
		end,
		kit.MenuField {
			-- (The node's menu, and for a parent its children's pins; the
			-- space's: every pin.)
			menu = function(id: string): { any }?
				if id == BACKGROUND then
					return { { label = locale.s("ds.orch.map.unpin_all"), icon = "pin", on_select = function()
						M.unpin_all()
					end } }
				end
				local i = fmodel.index[id]
				-- (A machine's hub has no menu of its own: its pins.)
				local base = if props.menu and not (i and fmodel.nodes[i].kind == "machine") then props.menu(id) else nil
				if i and fmodel.nodes[i].kind ~= "chat" then
					local out = table.clone(base or {})
					table.insert(out, { separator = true })
					table.insert(out, { label = locale.s("ds.orch.map.unpin_children"), icon = "pin", on_select = function()
						M.unpin_children(id)
					end })
					return out
				end
				return base
			end,
			register = props.register,
			key = "force",
			hit = function(px: number, py: number): string?
				return hit(px, py) or BACKGROUND
			end,
			center = function(id: string): (number?, number?)
				local x, y = screen_of(id)
				if x < -999 then
					return nil, nil
				end
				return x, y
			end,
			child = ui.div {
				class = "orch-force-field",
				id = "orch-force-field",
				relative = true,
				w_full = true,
				h = field_h,
				overflow = "hidden",
				rounded = "$radius.lg",
				on_visible = function(visible: boolean, _el: any)
					if visible ~= in_view:peek() then
						if visible and not in_view:peek() then
							open_t0 = os.clock()
							first_pending = true
						end
						in_view:set(visible)
					end
				end,
				on_press = function(e: any)
					on_press(e.local_x, e.local_y)
				end,
				on_pointer_move = function(e: any)
					on_move(e.local_x, e.local_y)
				end,
				on_release = function(e: any)
					on_release(e.local_x, e.local_y)
				end,
				on_hover = function(inside: boolean)
					in_field = inside
					if not inside and not press then
						leave_later()
					end
				end,
				on_wheel = function(e: any): boolean
					if e.ctrl or e.cmd then
						zoom_at(e.local_x, e.local_y, math.exp(-e.dy * 0.01))
						return true
					end
					return false
				end,
				on_pinch = function(e: any): boolean
					zoom_at(e.local_x, e.local_y, e.scale)
					return true
				end,
				ui.each(bands, band, { key = function(k: number): string
					return tostring(k)
				end, absolute = true, left = 0, top = 0, right = 0, bottom = 0, pointer_events = "none" }),
				ui.show(function(): boolean
					return on_screen() and glyphs_on:get()
				end, ui.div {
					class = "orch-force-glyphs",
					absolute = true,
					pointer_events = "none",
					left = function(): number
						return view:get().x
					end,
					top = function(): number
						return view:get().y
					end,
					w = 1,
					h = 1,
					ui.each(glyphed, glyph_el, { key = function(item: any): string
						return `{item.id}/{item.ring}`
					end, absolute = true, left = 0, top = 0, w = 1, h = 1, pointer_events = "none" }),
				}),
				ui.show(function(): boolean
					return on_screen() and settled:get()
				end, ui.div {
					class = "orch-force-layer",
					absolute = true,
					pointer_events = "none",
					left = function(): number
						return view:get().x
					end,
					top = function(): number
						return view:get().y
					end,
					w = 1,
					h = 1,
					ui.each(items, marker, { key = function(item: any): string
						return item.id
					end, absolute = true, left = 0, top = 0, w = 1, h = 1, pointer_events = "none" }),
				}),
				ui.div {
					class = "orch-force-anchor-at",
					absolute = true,
					left = function(): number
						local x, _, r = hover_box()
						anchor_at.x = x - r
						return x - r
					end,
					top = function(): number
						local _, y, r = hover_box()
						anchor_at.y = y - r
						return y - r
					end,
					ui.show(function(): boolean
						return M.hover:get() ~= nil
					end, anchor),
				},
				-- The machines, at the bottom left.
				ui.each(legend, legend_item, { key = function(item: any): string
					return item.id
				end, class = "orch-force-legend", absolute = true, left = 10, bottom = 8, gap = "$space.3", direction = "row", pointer_events = "none" }),
				-- The pivot, at the bottom right: orchestrators as the hubs (their
				-- machines the colour), or machines (their orchestrators').
				ui.div {
					class = "orch-force-pivot",
					absolute = true,
					right = 8,
					bottom = 6,
					Segmented {
						id = "orch-force-pivot",
						value = M.pivot,
						label = locale.t("ds.orch.map.pivot"),
						options = {
							{ value = "orchestrator", label = locale.t("ds.orch.map.pivot_orchestrators") },
							{ value = "machine", label = locale.t("ds.orch.map.pivot_machines") },
						},
					},
				},
				-- Fit, at the top right.
				ui.row {
					absolute = true,
					right = 8,
					top = 8,
					IconButton {
						id = "orch-force-fit",
						icon = "expand-arrows",
						label = locale.t("ds.orch.map.fit"),
						size = "sm",
						tone = "quiet",
						on_click = function()
							moved_view = false
							recentre()
							fit()
							recull()
						end,
					},
				},
			},
		},
		-- (No grip: a node is referenced from its menu, Mention in
		-- composer; the hover card alone shows what it is.)
	}
end)

return M

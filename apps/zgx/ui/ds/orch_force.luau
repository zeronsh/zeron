-- The Map's Graph as a web that holds still: the Brain dump in the middle,
-- each orchestrator a ringed hub on a strong line to it, its chats fanned
-- round it on strong lines of their own (each chat's todo as small dots
-- round it: filled done, hollow to do), thin weak lines between chats that
-- talk or command each other, a dashed "live" arc for a message in flight.
-- Colour says whose (an orchestrator's hue) or where (a machine's), as
-- "Color by" chooses; a legend under it names them.
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
local status = require("@self/status")
local locale = require("@self/locale")
local motion = require("@self/motion")
local hue = require("@self/orch_hue")
local kit = require("@self/orch_map_kit")
local model = require("@self/orch_map_model")
local Cells = require("@self/orch_cells")
local Ref = require("@self/orch_ref")
local sim = require("@self/orch_force_sim")
local fm = require("@self/orch_force_model")
local Graph2 = require("@self/orch_graph2")

local M = {}

-- Colour by: an orchestrator's hue, or a machine's. Kept on this device.
M.COLOR_BY = { "orchestrator", "machine" }
M.color_by = ui.persist("control_map_force_color", "orchestrator")
-- Where each node last settled, by id: { x, y } (kept on this device).
M.places = ui.persist("control_map_force_places", {} :: { [string]: { number } })
-- The node the pointer is on.
M.hover = ui.state("control_map_force_hover", nil :: string?)
-- For tests and numbers: builds, wakes, steps, draws, the settling.
M.stats = ui.state("control_map_force_stats", {} :: any)
M.zoom_by = function(_k: number) end
M.pan_by = function(_dx: number, _dy: number) end
M.drag_to = function(_id: string, _x: number, _y: number) end
M.current = nil :: any

-- A frame's share of the settling (seconds), and how often it's drawn
-- while settling (frames).
local STEP_BUDGET = 0.006
local DRAW_EVERY = 3
local PULSE_MS = 900
local PLACES_MAX = 4000
local TILE_H = kit.TILE_H
-- Names show from this zoom up (the important ones; others on hover).
local NAMES_ZOOM = 0.55
-- Machine hues (colour by machine), in the machines' order.
local MACHINE_HUES = { 38, 212, 282, 150, 330, 95, 190, 20, 250, 120 }
local R_BRAIN, R_HUB, R_CHAT = 19, 15, 9.5
local NEED: { [string]: boolean } = { waiting = true, input = true, approval = true }

local T_NAME: any = { size = 11.5, baseline = "top", align = "center" }
local T_HUB: any = { size = 12.5, weight = 600, baseline = "top", align = "center" }
local M_NAME = { size = 11.5 }
local M_HUB = { size = 12.5, weight = 600 }
local DASH = { dash = { 4, 4 } }
local ROUND = { cap = "round" :: "round", join = "round" :: "round" }
local T_BADGE: any = { size = 8.5, weight = 700, baseline = "middle", align = "center" }

-- The Brain's mark: a ghost (Material's expressive set), a dome over three
-- scallops, r across.
function M.ghost(x: number, y: number, r: number): any
	local top = y - 0.15 * r
	local p = ui.path():move(x - r, top):arc(x, top, r, 180, 180):line(x + r, y + 0.75 * r)
	local w = 2 * r / 3
	for k = 0, 2 do
		local x0 = x + r - k * w
		p = p:quad(x0 - w / 2, y + 0.75 * r + 0.42 * r, x0 - w, y + 0.75 * r)
	end
	return p:close()
end

-- An orchestrator's mark: a cookie with `n` scallops, r across.
function M.cookie(x: number, y: number, r: number, n: number): any
	local p = ui.path()
	local steps = 96
	for k = 0, steps do
		local t = k / steps * math.pi * 2
		local rr = r * (1 + 0.085 * math.cos(n * t))
		local px, py = x + math.cos(t) * rr, y + math.sin(t) * rr
		p = if k == 0 then p:move(px, py) else p:line(px, py)
	end
	return p:close()
end

local function fit_name(c: any, s: string, w: number, opts: any): string
	if c:measure_text(s, opts) <= w then
		return s
	end
	local n = utf8.len(s) or #s
	local lo, hi = 0, n
	while lo < hi do
		local mid = (lo + hi + 1) // 2
		local cut = string.sub(s, 1, (utf8.offset(s, mid + 1) or (#s + 1)) - 1) .. "…"
		if c:measure_text(cut, opts) <= w then
			lo = mid
		else
			hi = mid - 1
		end
	end
	return if lo == 0 then "" else string.sub(s, 1, (utf8.offset(s, lo + 1) or (#s + 1)) - 1) .. "…"
end

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
	local counts: any = { builds = 0, wakes = 0, steps = 0, draws = 0, overlay = 0, frames = 0, settled = 0, settle_ms = 0, first_ms = 0, nodes = 0, drawn = 0, worst_ms = 0 }
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
	local run = ui.use_state(0)
	local woke_at = 0
	local first_pending = false
	local open_t0 = os.clock()

	-- The chats as of the last world, for states (never a world built
	-- while drawing).
	local chats_now: { [string]: any } = {}
	local function state_of(id: string): string
		return model.status_of(chats_now[id])
	end

	-- Fitting the view to the web: its box in the field, at most 1.4x.
	local function fit()
		local x0, y0, x1, y1 = sim.bounds(s)
		local w, h = math.max(1, width:peek()), field_h()
		local pad = 70
		local z = math.clamp(math.min((w - pad * 2) / math.max(1, x1 - x0), (h - pad * 2 - 30) / math.max(1, y1 - y0)), 0.3, 1.4)
		view:set({ x = w / 2 - (x0 + x1) / 2 * z, y = (h - 30) / 2 - (y0 + y1) / 2 * z, zoom = z })
	end

	-- Settled: kept (by node), drawn, the markers to their places.
	local function done_settling()
		counts.settled += 1
		counts.settle_ms = (os.clock() - woke_at) * 1000
		-- (Kept for every node seen, so a node shown again, or another
		-- world's, opens where it was; at most PLACES_MAX, the web's own
		-- first.)
		local kept = table.clone(M.places:peek() or {})
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
		M.places:set(kept :: any)
		if not moved_view then
			fit()
		end
		settled:set(true)
		version:set(version:peek() + 1)
		tick:set(tick:peek() + 1)
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
			if not on_screen_now() then
				return false
			end
			local f0 = os.clock()
			local steps0 = s.steps
			local still = sim.run(s, STEP_BUDGET, os.clock)
			count("steps", s.steps - steps0)
			frames += 1
			count("frames")
			counts.worst_ms = math.max(counts.worst_ms, (os.clock() - f0) * 1000)
			-- (A long settling, past MAX_STEPS since waking: held where it is.)
			if not still and s.steps - counts.wake_steps > sim.MAX_STEPS then
				sim.sleep(s)
				still = true
			end
			if still then
				done_settling()
				return false
			end
			if frames % DRAW_EVERY == 0 then
				tick:set(tick:peek() + 1)
			end
			return nil
		end)
	end)
	counts.wake_steps = 0
	local function wake(ids: { number }?)
		sim.wake(s, ids)
		counts.wake_steps = s.steps
		woke_at = os.clock()
		count("wakes")
		settled:set(false)
		run:set(run:peek() + 1)
	end

	-- A new model when what's shown changes: the same nodes and links, its
	-- states and names taken in place; else a new web, from where its
	-- nodes were (this one, else kept), waking only what's new and round it.
	local function rebuild(world: any, comms: { any })
		local next_m = fm.build(world, comms, world.clock or os.time())
		if next_m.sig == fmodel.sig then
			fmodel.arcs = next_m.arcs
			for i, n in next_m.nodes do
				local o = fmodel.nodes[i]
				o.title, o.fade, o.done, o.total, o.hue, o.device = n.title, n.fade, n.done, n.total, n.hue, n.device
			end
			fmodel.machines = next_m.machines
			version:set(version:peek() + 1)
			tick:set(tick:peek() + 1)
			return
		end
		count("builds")
		local t0 = os.clock()
		local nodes: { sim.Node } = table.create(#next_m.nodes) :: any
		for i, n in next_m.nodes do
			nodes[i] = { id = n.id, parent = if n.parent > 0 then n.parent else nil, mass = if n.kind == "brain" then 4 elseif n.kind == "orchestrator" then 2.5 else 1, fixed = n.kind == "brain" }
		end
		local links: { sim.Link } = table.create(#next_m.links) :: any
		for i, l in next_m.links do
			links[i] = { a = l.a, b = l.b, strong = l.strong, hub = l.hub }
		end
		local next_s = sim.new(nodes, links, 7)
		-- (Where they were: this web, else kept from before.)
		local from: { [string]: any } = table.clone(M.places:peek() or {})
		for i = 1, s.n do
			from[s.ids[i]] = { s.x[i], s.y[i] }
		end
		local fresh = sim.place(next_s, from)
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
		rebuild(world, comms)
	end)

	-- The palette, once per theme and colour mode: no string made while
	-- drawing.
	local pal: any = nil
	local pal_theme: any, pal_by: any, pal_ver = nil, nil, -1
	local node_ink: { string } = {}
	local node_line: { string } = {}
	local node_halo: { string } = {}
	local function palette(): any
		local theme = ui.theme.current:get()
		local by = M.color_by:get()
		local ver = version:get()
		if pal and pal_theme == theme and pal_by == by and pal_ver == ver then
			return pal
		end
		pal_theme, pal_by, pal_ver = theme, by, ver
		local ink = kit.ink("text.primary")
		local r, g, b = tonumber(string.sub(ink, 2, 3), 16) or 0, tonumber(string.sub(ink, 4, 5), 16) or 0, tonumber(string.sub(ink, 6, 7), 16) or 0
		local dark = r + g + b > 384
		pal = {
			dark = dark,
			fg = kit.ink("text.primary", 0.92),
			secondary = kit.ink("text.primary", 0.6),
			bg = kit.ink("background"),
			halo = kit.ink("text.primary", if dark then 0.06 else 0.05),
			brain = kit.ink("text.primary", if dark then 0.88 else 0.82),
			body = kit.ink("text.primary", if dark then 0.1 else 0.07),
			glyph = kit.ink("text.primary", if dark then 0.86 else 0.8),
			idle = kit.ink("text.primary", 0.22),
			weak = kit.ink("text.primary", 0.14),
			live = kit.ink("text.primary", 0.78),
			working = kit.ink("zeron.status.plan"),
			ask = kit.ink("zeron.status.input"),
			done = kit.ink("zeron.status.done"),
			failed = kit.ink("zeron.status.failed"),
			label_bg = kit.ink("background", 0.8),
			lit = kit.ink("text.primary", 0.85),
			-- (A fading chat's ring and glyph, by its strength in tenths.)
			fade = {},
		}
		for k = 1, 10 do
			pal.fade[k] = kit.ink("text.primary", 0.22 * k / 10)
		end
		-- Each node's colour: by its orchestrator's hue, or its machine's.
		table.clear(node_ink)
		table.clear(node_line)
		table.clear(node_halo)
		local mhue: { [string]: number } = {}
		for k, m in fmodel.machines do
			mhue[m.id] = MACHINE_HUES[(k - 1) % #MACHINE_HUES + 1]
		end
		for i, n in fmodel.nodes do
			local h = if by == "machine" then mhue[n.device or ""] else n.hue
			node_ink[i] = if h then hue.canvas(h, 1) else pal.secondary
			node_line[i] = if h then hue.canvas(h, if n.kind == "orchestrator" then 0.55 else 0.42) else pal.weak
			node_halo[i] = if h then hue.canvas(h, 0.14) else pal.halo
		end
		return pal
	end

	-- The settled web's drawing, made once when it rests (its shapes, its
	-- glyphs, its names and where they go): never a frame of the settling,
	-- which draws plain discs.
	local shapes: { [number]: any } = {}
	local glyphs: { [number]: any } = {}
	local glyph_fill: { [number]: boolean } = {}
	local rings: { [number]: any } = {}
	local names: { string } = {}
	local label_x: { [number]: number } = {}
	local label_y: { [number]: number } = {}
	local made_for = -1
	local function sides(i: number): number
		local crew = 0
		for _, j in s.nbrs[i] do
			if s.parent[j] == i then
				crew += 1
			end
		end
		return if crew <= 3 then 4 elseif crew <= 6 then 6 elseif crew <= 10 then 7 elseif crew <= 18 then 9 else 12
	end
	local function make(c: any)
		local key = version:peek()
		if made_for == key or not settled:peek() then
			return
		end
		made_for = key
		table.clear(shapes)
		table.clear(glyphs)
		table.clear(glyph_fill)
		table.clear(rings)
		table.clear(names)
		table.clear(label_x)
		table.clear(label_y)
		-- Names placed greedily, the important first, none over a mark or
		-- another name (an orchestrator's on the Brain's side, away from
		-- its fan).
		local boxes: { { number } } = {}
		local function free(x0: number, y0: number, x1: number, y1: number): boolean
			for _, b in boxes do
				if not (x1 < b[1] or x0 > b[3] or y1 < b[2] or y0 > b[4]) then
					return false
				end
			end
			return true
		end
		for i, n in fmodel.nodes do
			local r = if n.kind == "brain" then R_BRAIN elseif n.kind == "orchestrator" then R_HUB else R_CHAT + 3
			table.insert(boxes, { s.x[i] - r, s.y[i] - r, s.x[i] + r, s.y[i] + r })
		end
		for i, n in fmodel.nodes do
			local x, y = s.x[i], s.y[i]
			if n.kind == "brain" then
				shapes[i] = M.ghost(x, y, R_BRAIN)
				names[i] = locale.s("ds.orch.map.brain")
				label_x[i], label_y[i] = x, y + R_BRAIN + 9
				local w = c:measure_text(names[i], M_HUB)
				table.insert(boxes, { x - w / 2 - 3, y + R_BRAIN + 8, x + w / 2 + 3, y + R_BRAIN + 26 })
			elseif n.kind == "orchestrator" then
				shapes[i] = M.cookie(x, y, R_HUB, sides(i))
				names[i] = fit_name(c, n.title, 170, M_HUB)
				local w = c:measure_text(names[i], M_HUB)
				local p = n.parent
				local dx, dy = (if p > 0 then s.x[p] else x) - x, (if p > 0 then s.y[p] else y + 1) - y
				local d = math.max(1, math.sqrt(dx * dx + dy * dy))
				local off = R_HUB + 12 + math.abs(dx / d) * w / 2 + math.abs(dy / d) * 6
				label_x[i], label_y[i] = x + dx / d * off, y + dy / d * off - 8
				table.insert(boxes, { label_x[i] - w / 2 - 3, label_y[i], label_x[i] + w / 2 + 3, label_y[i] + 17 })
			else
				glyphs[i], glyph_fill[i] = Graph2.glyph(n.provider, x, y)
				rings[i] = ui.path():arc(x, y, R_CHAT + 3.5, 0, 360)
				names[i] = fit_name(c, n.title, 150, M_NAME)
			end
		end
		-- Chats' names: those asking for the person (others on hover).
		for i, n in fmodel.nodes do
			if n.kind == "chat" and NEED[state_of(n.id)] and names[i] ~= "" then
				local w = c:measure_text(names[i], M_NAME)
				local x, y = s.x[i], s.y[i] + R_CHAT + 6
				if free(x - w / 2 - 3, y, x + w / 2 + 3, y + 15) then
					label_x[i], label_y[i] = x, y
					table.insert(boxes, { x - w / 2 - 3, y, x + w / 2 + 3, y + 15 })
				end
			end
		end
	end
	local names_shown = ui.computed(function(): boolean
		return view:get().zoom >= NAMES_ZOOM
	end)

	-- What's drawn: the view's reach (world), a field past it each way;
	-- drawn again only when the view nears its edge.
	local cull = ui.use_state({ x0 = -1e9, y0 = -1e9, x1 = 1e9, y1 = 1e9 })
	local function recull()
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
	local function draw_web(c: any, top: number, bottom: number)
		local p = palette()
		local v = view:peek()
		local b = cull:get()
		local still = settled:peek()
		make(c)
		local full = still and made_for == version:peek()
		local by0, by1 = (top - v.y) / v.zoom, (bottom - v.y) / v.zoom
		local x0, y0, x1, y1 = b.x0, math.max(b.y0, by0 - 60), b.x1, math.min(b.y1, by1 + 60)
		local x, y = s.x, s.y
		local nodes = fmodel.nodes
		-- Links: weak (hairlines), then strong (a hub's heavier).
		for _, l in fmodel.links do
			local ax, ay, bx, byy = x[l.a], y[l.a], x[l.b], y[l.b]
			if math.max(ax, bx) < x0 or math.min(ax, bx) > x1 or math.max(ay, byy) < y0 or math.min(ay, byy) > y1 then
				continue
			end
			if not l.strong then
				c:line(ax, ay, bx, byy, p.weak, 0.8)
			end
		end
		for _, l in fmodel.links do
			if l.strong then
				local ax, ay, bx, byy = x[l.a], y[l.a], x[l.b], y[l.b]
				if math.max(ax, bx) < x0 or math.min(ax, bx) > x1 or math.max(ay, byy) < y0 or math.min(ay, byy) > y1 then
					continue
				end
				c:line(ax, ay, bx, byy, node_line[l.b], if l.hub then 2.2 else 1.2)
			end
		end
		-- Marks.
		local show_names = names_shown:get() and full
		local drawn = 0
		for i, n in nodes do
			local nx, ny = x[i], y[i]
			if nx < x0 - 40 or nx > x1 + 40 or ny < y0 - 40 or ny > y1 + 40 then
				continue
			end
			drawn += 1
			if n.kind == "brain" then
				c:circle(nx, ny, R_BRAIN * 1.75, p.halo)
				if full then
					c:fill(shapes[i], p.brain)
				else
					c:circle(nx, ny, R_BRAIN, p.brain)
				end
			elseif n.kind == "orchestrator" then
				c:circle(nx, ny, R_HUB * 1.9, node_halo[i])
				if full then
					c:fill(shapes[i], node_ink[i])
				else
					c:circle(nx, ny, R_HUB, node_ink[i])
				end
				c:circle(nx, ny, 3.6, p.bg)
			else
				local st = state_of(n.id)
				local faded = n.fade < 10
				c:circle(nx, ny, R_CHAT, p.body)
				if full and glyphs[i] then
					local gi = if faded then p.fade[n.fade] else p.glyph
					if glyph_fill[i] then
						c:fill(glyphs[i], gi)
					else
						c:stroke(glyphs[i], gi, 1.2, ROUND)
					end
				end
				local R = R_CHAT + 3.5
				if NEED[st] then
					c:circle(nx, ny, R + 0.9, p.ask)
					c:circle(nx, ny, R - 0.9, p.bg)
					c:circle(nx, ny, R_CHAT, p.body)
					if full then
						if glyphs[i] then
							if glyph_fill[i] then
								c:fill(glyphs[i], p.glyph)
							else
								c:stroke(glyphs[i], p.glyph, 1.2, ROUND)
							end
						end
						c:circle(nx + 9.5, ny - 9.5, 6, p.ask)
						T_BADGE.color = p.bg
						c:text("?", nx + 9.5, ny - 9.5, T_BADGE)
					end
				elseif st == "working" then
					-- (Its dashed ring is the overlay's, stepping.)
					if not full then
						c:circle(nx, ny, R + 0.8, p.working)
						c:circle(nx, ny, R - 0.8, p.bg)
						c:circle(nx, ny, R_CHAT, p.body)
					end
				elseif full and rings[i] then
					local ink = if st == "done" then p.done elseif st == "failed" then p.failed elseif faded then p.fade[n.fade] else p.idle
					c:stroke(rings[i], ink, if st == "idle" then 1.1 else 1.5)
				end
			end
			if show_names and label_x[i] then
				local t = (if n.kind == "chat" then T_NAME else T_HUB) :: any
				t.color = if n.kind == "chat" then p.secondary else p.fg
				c:text(names[i], label_x[i], label_y[i], t)
			end
		end
		-- Live: a dashed arc for each message in flight.
		if full then
			for _, a in fmodel.arcs do
				local ax, ay, bx, byy = x[a.a], y[a.a], x[a.b], y[a.b]
				local mx, my = (ax + bx) / 2, (ay + byy) / 2
				local dx, dy = bx - ax, byy - ay
				c:stroke(ui.path():move(ax, ay):quad(mx - dy * 0.18, my + dx * 0.18, bx, byy), p.live, 1.4, DASH)
			end
		end
		return drawn
	end

	-- The overlay: the working rings (dashed, stepping on the shared status
	-- clock: a tenth of a second a step, none while none shows), what's lit
	-- (the node under the pointer, its links, its name), the dots running
	-- lines.
	local pulses: { any } = {}
	local ptick = ui.use_state(0)
	local phase = 0
	-- (Eight dashes round a ring of radius R_CHAT + 3.5.)
	local RING_SEG = 2 * math.pi * (R_CHAT + 3.5) / 8
	local RING_DASH: any = { dash = { RING_SEG * 0.56, RING_SEG * 0.44 }, dash_offset = 0, cap = "round" }
	local function draw_over(c: any)
		local p = palette()
		local x, y = s.x, s.y
		if settled:peek() and made_for == version:peek() then
			RING_DASH.dash_offset = phase
			for i, n in fmodel.nodes do
				if rings[i] and state_of(n.id) == "working" then
					c:stroke(rings[i], p.working, 1.8, RING_DASH)
				end
			end
		end
		local over = M.hover:get()
		local i = over and fmodel.index[over]
		if i then
			local n = fmodel.nodes[i]
			for _, l in fmodel.links do
				if l.a == i or l.b == i then
					c:line(x[l.a], y[l.a], x[l.b], y[l.b], p.lit, if l.strong then 1.8 else 1)
				end
			end
			local r = if n.kind == "brain" then R_BRAIN elseif n.kind == "orchestrator" then R_HUB else R_CHAT + 3.5
			c:circle(x[i], y[i], r + 4, p.lit)
			c:circle(x[i], y[i], r + 2.5, p.bg)
			if n.kind == "chat" and not label_x[i] and names[i] and names[i] ~= "" then
				T_NAME.color = p.fg
				c:rect(x[i] - 80, y[i] + R_CHAT + 5, 160, 16, p.label_bg, 4)
				c:text(names[i], x[i], y[i] + R_CHAT + 6, T_NAME)
			end
		end
		local now = ui.now()
		for _, q in pulses do
			local t = (now - q.t0) / PULSE_MS
			if t >= 0 and t <= 1 then
				local a, b = q.a, q.b
				local e = 1 - (1 - t) ^ 3
				local px, py = x[a] + (x[b] - x[a]) * e, y[a] + (y[b] - y[a]) * e
				c:circle(px, py, 5, p.halo)
				c:circle(px, py, 3, p.live)
			end
		end
	end
	-- The working rings step while some show, on screen and settled.
	local field_el: any = nil
	local step_key = {}
	local stepping = false
	ui.effect(function()
		version:get()
		local want = on_screen() and settled:get() and status.stepped() and not motion.reduced_now()
		local any = false
		if want then
			for _, n in fmodel.nodes do
				if n.kind == "chat" and state_of(n.id) == "working" then
					any = true
					break
				end
			end
		end
		if any and field_el and not stepping then
			stepping = true
			status.step(step_key, field_el, function(_el: any, now: number)
				-- (Eight even dashes round the ring, a dash's eighth a step.)
				phase = -(math.floor(now / 100) % 8) * RING_SEG / 8
				ptick:set(ptick:peek() + 1)
			end)
		elseif not any and stepping then
			stepping = false
			status.unstep(step_key)
		end
	end)
	ui.on_cleanup(function()
		if stepping then
			status.unstep(step_key)
		end
	end)
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
		view:set({ x = x, y = y, zoom = zoom })
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
			local r = (if n.kind == "brain" then R_BRAIN elseif n.kind == "orchestrator" then R_HUB else R_CHAT) + 5 / z
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
		if not i or s.fixed[i] then
			return
		end
		s.held[i] = true
		s.x[i], s.y[i] = wx, wy
		if sim.still(s) or not s.awake[i] then
			wake({ i })
		end
		tick:set(tick:peek() + 1)
	end
	M.drag_to = function(id: string, wx: number, wy: number)
		drag(id, wx, wy)
		local i = fmodel.index[id]
		if i then
			s.held[i] = s.fixed[i]
		end
	end
	local press: any = nil
	local grip_hot = ui.use_state(false)
	local in_field = false
	local function on_press(px: number, py: number)
		press = { id = hit(px, py), x = px, y = py, moved = false, view = view:peek() }
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
	local function open(id: string)
		local i = fmodel.index[id]
		local n = i and fmodel.nodes[i]
		if not n then
			return
		end
		if n.kind == "chat" then
			props.on_open_chat(id)
		else
			props.on_open_orchestrator(id)
		end
	end
	local function on_release(px: number, py: number)
		local p = press
		press = nil
		if not p then
			return
		end
		if not p.moved then
			local id = hit(px, py)
			if id then
				open(id)
			end
		elseif p.id then
			local i = fmodel.index[p.id]
			if i then
				s.held[i] = s.fixed[i]
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
		local r = if n.kind == "brain" then R_BRAIN elseif n.kind == "orchestrator" then R_HUB else R_CHAT
		return s.x[i] * v.zoom + v.x, s.y[i] * v.zoom + v.y, r * v.zoom
	end
	local function ref_of(id: string): any
		local i = fmodel.index[id]
		return kit.ref_of(props.world, id, if i then fmodel.nodes[i].title else nil)
	end

	-- The grip beside the node under the pointer: dragged, a reference.
	local grip_for = ui.use_state(nil :: string?)
	ui.effect(function()
		local id = M.hover:get()
		if id and id ~= grip_for:peek() then
			grip_for:set(id)
		end
	end)
	local function grip_at(): (number, number)
		view:get()
		tick:get()
		local id = grip_for:get()
		if not id then
			return -1000, -1000
		end
		local x, y, r = screen_of(id)
		return x + r + 6, y
	end
	local grip = ui.div {
		id = "orch-force-grip",
		class = "orch-force-grip",
		label = locale.t("ds.orch.map.reference"),
		absolute = true,
		left = function(): number
			local x = grip_at()
			return x
		end,
		top = function(): number
			local _, y = grip_at()
			return y - 10
		end,
		w = 20,
		h = 20,
		rounded = "$radius.sm",
		items = "center",
		justify = "center",
		color = "$text.secondary",
		bg = "$surface.raised",
		border = 1,
		border_color = "$border",
		hover = { bg = "$hover" },
		on_hover = function(inside: boolean)
			grip_hot:set(inside)
			if not inside then
				leave_later()
			end
		end,
		on_drag = function(): any
			local id = grip_for:peek()
			local r = if id then ref_of(id) else nil
			return if r then { ref = r } else nil
		end,
		drag_preview = function(payload: any): Node
			local r = Ref.of(payload)
			return if r then Ref.Pill(r) else ui.div {}
		end,
		drag_preview_anchor = { 0, 0.5 },
		drag_preview_offset = { 12, 0 },
		Icon { name = "drag-handle", size = 12 },
	}

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
		tick:get()
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
					local n = draw_web(c, top, bottom())
					count("draws")
					counts.drawn = n
					local ms = (os.clock() - t0) * 1000
					counts.draw_ms = ms
					if first_pending then
						first_pending = false
						counts.first_ms = (os.clock() - open_t0) * 1000
					end
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
					tick:get()
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
			menu = props.menu,
			register = props.register,
			key = "force",
			hit = hit,
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
				on_visible = function(visible: boolean, el: any)
					field_el = el
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
					end, absolute = true, left = 0, top = 0, pointer_events = "none" }),
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
				-- Fit, at the top right (Colour by is the Map's menu's).
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
							fit()
							recull()
						end,
					},
				},
			},
		},
		ui.show(function(): boolean
			return M.hover:get() ~= nil or grip_hot:get()
		end, grip),
	}
end)

return M

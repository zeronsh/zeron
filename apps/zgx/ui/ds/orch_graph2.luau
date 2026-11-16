-- The Map's Graph, laid out rather than settled (shown as "Graph (new)"
-- while the first Graph, ds/orch_map_graph, stays): every machine a region
-- of the field, sized by what it runs (ds/orch_graph2_layout); in it, each
-- orchestrator in its hue with its crew in a grid, its line coming down a
-- trunk and along a rail to each; the person's own threads open rings
-- after them. Crew led from another machine gather under a ghost of their
-- lead, one dashed line in the gaps to it. A dot runs a line as agents
-- talk.
--
-- Nothing moves on its own: the layout is worked out once for what's on
-- the Map and its width; a change eases what moved to where it now sits
-- (250 ms). A pan or a zoom moves the drawing as it stands (the canvas's
-- `view`): nothing is laid out or drawn again for it. Drag the space to
-- pan; pinch or ⌘/Ctrl-scroll to zoom (a plain scroll is the page's).
-- The pointer on a thread lights its line and shows its card, with a grip
-- beside it to drag a reference to it; a click opens it; a secondary click
-- its menu.
--
--   Graph2.View { world, chat, comms, on_open_chat, on_open_orchestrator,
--                 menu?, register? }

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
local MachineHead = require("@self/orch_machine_head")
local Ref = require("@self/orch_ref")
local L = require("@self/orch_graph2_layout")
local Scene = require("@self/orch_graph2_scene")

local M = {}

local PULSE_MS = 900
local EASE_MS = 250
-- Below this zoom the names step out (the marks and lines stay).
local NAMES_ZOOM = 0.6
local FONT = 12
local LEAD_FONT = 12.5
local LINE_DASH = { dash = { 4, 3 } }
local GLYPH_STROKE = { cap = "round" :: "round", join = "round" :: "round" }
local LIT_STROKE = { cap = "round" :: "round", join = "round" :: "round" }

-- How a thread's mark is drawn: a dot in its state's colour, or its
-- provider's glyph (Claude, OpenAI, Gemini; a plain mark for others) in
-- a ring of that colour, where there's room (a dot when zoomed out or in
-- a tight grid). Kept on this device.
M.STYLES = { "dots", "provider" }
M.node_style = ui.persist("control_map_graph2_node_style", "dots")
-- Glyphs from this zoom and cell height up.
local GLYPH_ZOOM, GLYPH_CELL = 0.8, 22

-- The node the pointer is on (its id; a machine's "dev:<id>").
M.hover = ui.state("control_map_graph2_hover", nil :: string?)
-- For tests: how many times the layout was worked out and the base drawn,
-- and the last layout's size.
M.stats = ui.state("control_map_graph2_stats", { layouts = 0, draws = 0, overlay = 0, w = 0, h = 0, layout_ms = 0 } :: any)
M.zoom_by = function(_k: number) end
M.pan_by = function(_dx: number, _dy: number) end
-- (The layout showing, for tests: positions by id.)
M.current = nil :: L.Layout?

-- A provider's glyph at (x, y), about 9 points across, monochrome: its
-- path, and whether it's filled (else stroked).
function M.glyph(provider: string, x: number, y: number): (any, boolean)
	if provider == "claude" then
		-- (A burst: eight rays.)
		local p = ui.path()
		for k = 0, 7 do
			local a = math.rad(k * 45 + 22.5)
			p = p:move(x + math.cos(a) * 1.3, y + math.sin(a) * 1.3):line(x + math.cos(a) * 4.3, y + math.sin(a) * 4.3)
		end
		return p, false
	elseif provider == "openai" then
		-- (A hexagon, a point up.)
		local p = ui.path()
		for k = 0, 5 do
			local a = math.rad(k * 60 - 90)
			local px, py = x + math.cos(a) * 4, y + math.sin(a) * 4
			p = if k == 0 then p:move(px, py) else p:line(px, py)
		end
		return p:close(), false
	elseif provider == "gemini" then
		-- (A four-pointed spark.)
		local k = 4.6
		return ui.path():move(x, y - k):quad(x, y, x + k, y):quad(x, y, x, y + k):quad(x, y, x - k, y):quad(x, y, x, y - k):close(), true
	end
	-- (Any other: a small square.)
	return ui.path():rect(x - 2.6, y - 2.6, 5.2, 5.2, 1), false
end

local function ease(t: number): number
	t = math.clamp(t, 0, 1)
	return 1 - (1 - t) ^ 3
end

-- A name cut to `w` points at a word's end where it can: once per layout.
local function fit_name(c: any, s: string, w: number, opts: any): string
	if w <= 8 then
		return ""
	end
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
	if lo == 0 then
		return ""
	end
	return string.sub(s, 1, (utf8.offset(s, lo + 1) or (#s + 1)) - 1) .. "…"
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
	local field_h = ui.use_state(L.MIN_H)
	local view = ui.use_state({ x = 0, y = 0, zoom = 1 })
	-- The layout showing; `version` bumps when it's a new one (or names
	-- changed in place).
	local layout: L.Layout = L.layout({ machines = {} }, 1)
	local version = ui.use_state(0)
	-- Whether it's on screen: in view (the field's `on_visible`) in a
	-- window that shows. Off screen nothing of it runs: no layout, no
	-- drawing, no dot, no ease, no marker following a thread; back, it
	-- takes the world as it is then.
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
	-- Where each node and machine was before a change, for the ease.
	local from_x: { [string]: number } = {}
	local from_y: { [string]: number } = {}
	local anim = ui.use_state(1)
	local anim_t0 = 0
	local ease_run = ui.use_state(0)
	ui.effect(function()
		if ease_run:get() == 0 then
			return
		end
		ui.every(16, function(): boolean?
			local t = (ui.now() - anim_t0) / EASE_MS
			if t >= 1 or not on_screen_now() then
				anim:set(1)
				return false
			end
			anim:set(t)
			return nil
		end)
	end)
	-- Each node's name as fitted to its cell, by index (once per layout).
	local names: { string } = {}
	local names_for = -1
	-- The drawing's reach (world): what lies outside isn't drawn until the
	-- view comes near it.
	local cull = ui.use_state({ x0 = -1e9, y0 = -1e9, x1 = 1e9, y1 = 1e9 })
	local pulses: { any } = {}
	local ptick = ui.use_state(0)

	-- (Counts for tests, published once after the draws of a frame. Timers
	-- belong to the effect run that starts them, so each runs from an
	-- effect of its own: publishing, the ease, the dots.)
	local counts = { layouts = 0, draws = 0, overlay = 0, drawn = 0, w = 0, h = 0, layout_ms = 0, draw_kb = 0, overlay_kb = 0, faded = 0, glyphs = 0, relayout_runs = 0 }
	local dirty = ui.use_state(false)
	local function count(key: string)
		(counts :: any)[key] += 1
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

	-- The chats as of the last world (taken when it changes, so drawing
	-- never builds one).
	local chats_now: { [string]: any } = {}
	-- Each thread's state as shown, put here by its marker as it changes
	-- (`status_ver` says one did): the drawing reads one state, not one
	-- a thread.
	local status_by: { [string]: string } = {}
	local status_ver = ui.use_state(0)
	local function state_of(id: string): string
		return status_by[id] or model.status_of(chats_now[id])
	end

	-- The palette, made once per theme: no string built while drawing.
	local pal: any = nil
	local pal_theme: any = nil
	local hue_ink: { [number]: string } = {}
	local hue_line: { [number]: string } = {}
	local hue_soft: { [number]: string } = {}
	local function palette(): any
		local theme = ui.theme.current:get()
		if pal and pal_theme == theme then
			return pal
		end
		pal_theme = theme
		table.clear(hue_ink)
		table.clear(hue_line)
		table.clear(hue_soft)
		local ink = kit.ink("text.primary")
		local r, g, b = tonumber(string.sub(ink, 2, 3), 16) or 0, tonumber(string.sub(ink, 4, 5), 16) or 0, tonumber(string.sub(ink, 6, 7), 16) or 0
		local dark = r + g + b > 384
		pal = {
			dark = dark,
			region = kit.ink("text.primary", if dark then 0.035 else 0.025),
			edge = kit.ink("text.primary", if dark then 0.09 else 0.1),
			name = kit.ink("text.primary", 0.92),
			quiet = kit.ink("text.primary", if dark then 0.5 else 0.55),
			idle = kit.ink("text.primary", if dark then 0.32 else 0.3),
			plain = kit.ink("text.primary", if dark then 0.45 else 0.4),
			bg = kit.ink("background"),
			working = kit.ink("zeron.status.working"),
			waiting = kit.ink("zeron.status.waiting"),
			input = kit.ink("zeron.status.input"),
			approval = kit.ink("zeron.status.approval"),
			failed = kit.ink("zeron.status.failed"),
			done = kit.ink("zeron.status.done", 0.75),
			halo_input = kit.ink("zeron.status.input", 0.2),
			halo_approval = kit.ink("zeron.status.approval", 0.2),
			halo_failed = kit.ink("zeron.status.failed", 0.2),
			lit = kit.ink("text.primary", 0.85),
			-- (A fading thread's dot and name, by its strength in tenths.)
			fade_dot = {},
			fade_name = {},
			glyph = kit.ink("text.primary", if dark then 0.88 else 0.8),
			glyph_quiet = kit.ink("text.primary", if dark then 0.45 else 0.42),
		}
		for k = 1, 10 do
			pal.fade_dot[k] = kit.ink("text.primary", (if dark then 0.32 else 0.3) * k / 10)
			pal.fade_name[k] = kit.ink("text.primary", (if dark then 0.5 else 0.55) * k / 10)
		end
		return pal
	end
	local function hink(h: number?): string
		local k = h or -1
		local v = hue_ink[k]
		if not v then
			v = if h then hue.canvas(h, 1) else (pal :: any).quiet
			hue_ink[k] = v
		end
		return v
	end
	local function hline(h: number?): string
		local k = h or -1
		local v = hue_line[k]
		if not v then
			v = if h then hue.canvas(h, 0.7) else (pal :: any).edge
			hue_line[k] = v
		end
		return v
	end
	local function hsoft(h: number?): string
		local k = h or -1
		local v = hue_soft[k]
		if not v then
			v = if h then hue.canvas(h, 0.28) else (pal :: any).edge
			hue_soft[k] = v
		end
		return v
	end

	-- The cut-out paths, made once per layout: regions, dashed lines.
	local region_paths: { any } = {}
	local line_paths: { any } = {}
	local brain_paths: { [number]: any } = {}
	local glyph_paths: { [number]: any } = {}
	local glyph_fill: { [number]: boolean } = {}
	local function make_paths()
		table.clear(region_paths)
		table.clear(line_paths)
		table.clear(brain_paths)
		table.clear(glyph_paths)
		table.clear(glyph_fill)
		for i, n in layout.nodes do
			if not n.lead then
				glyph_paths[i], glyph_fill[i] = M.glyph(n.provider, n.x, n.y)
			end
		end
		for i, n in layout.nodes do
			if n.lead and n.kind == "brain" then
				local k = 6.5
				brain_paths[i] = ui.path():move(n.x, n.y - k):line(n.x + k, n.y):line(n.x, n.y + k):line(n.x - k, n.y):close()
			end
		end
		for i, m in layout.machines do
			local r = m.rect
			region_paths[i] = ui.path():rect(r.x + 0.5, r.y + 0.5, r.w - 1, r.h - 1, 10)
		end
		for i, ln in layout.lines do
			local p = ui.path():move(ln.pts[1], ln.pts[2])
			for k = 3, #ln.pts, 2 do
				p = p:line(ln.pts[k], ln.pts[k + 1])
			end
			line_paths[i] = p
		end
	end

	-- A new layout when what's on the Map or the width changes; the same
	-- one with names changed is taken in place.
	local function relayout(world: any, w: number)
		if w <= 0 then
			return
		end
		local t0 = os.clock()
		local scene = Scene.of(world)
		local next_l = L.layout(scene, w)
		local ms = (os.clock() - t0) * 1000
		if next_l.sig == layout.sig then
			local renamed = false
			local marks = false
			for i, n in next_l.nodes do
				local old = layout.nodes[i]
				if old and old.title ~= n.title then
					old.title = n.title
					renamed = true
				end
				if old and (old.fade ~= n.fade or old.provider ~= n.provider) then
					old.fade, old.provider = n.fade, n.provider
					marks = true
				end
			end
			if marks then
				make_paths()
				version:set(version:peek() + 1)
			end
			for i, m in next_l.machines do
				local old = layout.machines[i]
				if old and old.name ~= m.name then
					old.name = m.name
					renamed = true
				end
			end
			if renamed then
				names_for = -1
				version:set(version:peek() + 1)
			end
			return
		end
		-- (Where things were, to ease from; nothing for a first layout.)
		table.clear(from_x)
		table.clear(from_y)
		local first = #layout.nodes == 0
		if not first then
			for _, n in layout.nodes do
				from_x[n.id], from_y[n.id] = n.x, n.y
			end
			for _, m in layout.machines do
				from_x[`dev:{m.id}`], from_y[`dev:{m.id}`] = m.rect.x, m.rect.y
			end
		end
		layout = next_l
		M.current = next_l
		names_for = -1
		make_paths()
		field_h:set(next_l.h)
		counts.w, counts.h, counts.layout_ms = next_l.w, next_l.h, ms
		count("layouts")
		version:set(version:peek() + 1)
		if first or motion.reduced_now() then
			anim:set(1)
			return
		end
		anim_t0 = ui.now()
		anim:set(0)
		ease_run:set(ease_run:peek() + 1)
	end

	-- (Laid out again when the world or the width changes: what the Map
	-- shows is the world's, its "Show chats" and its clock, which moves on
	-- each minute; the same layout, a fading chat fainter, is taken in
	-- place.)
	--
	-- (The world's shape: what it holds but the machines' loads, each by
	-- identity. The loads change every few seconds; that reaches the
	-- headings' figures and rings alone, never the layout.)
	local shape_world: any = nil
	local shape_n = 0
	local SHAPE_KEYS = { "now", "chats", "devices", "projects", "here", "show", "clock" }
	-- (Lists, the projects and devices, by what's in them: a list read
	-- again may be a fresh table holding the same.)
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
	local shape = ui.computed(function(): number
		if not on_screen() then
			return shape_n
		end
		local world = props.world()
		local changed = shape_world == nil
		if not changed then
			for _, k in SHAPE_KEYS do
				if not same(k, world[k], shape_world[k]) then
					changed = true
					break
				end
			end
		end
		if changed then
			shape_n += 1
			shape_world = world
		end
		return shape_n
	end)
	ui.effect(function()
		if not on_screen() then
			return
		end
		shape:get()
		local world = shape_world
		if not world then
			return
		end
		chats_now = world.chats or {}
		local w = width:get()
		count("relayout_runs")
		relayout(world, w)
	end)

	-- A node's place as drawn now (mid-ease, between where it was and is).
	local function place(n: L.Node, e: number): (number, number)
		if e >= 1 then
			return n.x, n.y
		end
		local fx = from_x[n.id]
		if not fx then
			return n.x, n.y
		end
		return fx + (n.x - fx) * e, from_y[n.id] + (n.y - from_y[n.id]) * e
	end

	-- Text options, made once and set before each use. (zgx reads a text's
	-- options into a table of its own each call, ~0.75 KB: names are drawn
	-- once the ease is done, never in its frames.)
	local T_NAME: any = { size = FONT, baseline = "middle" }
	local T_LEAD: any = { size = LEAD_FONT, weight = 600, baseline = "middle" }
	local M_NAME = { size = FONT }
	local M_LEAD = { size = LEAD_FONT, weight = 600 }

	local function fit_names(c: any, glyphs: boolean)
		local key = version:peek() * 2 + (if glyphs then 1 else 0)
		if names_for == key then
			return
		end
		names_for = key
		table.clear(names)
		for i, n in layout.nodes do
			if n.lead then
				local t = if n.ghost then `↗ {n.title}` else n.title
				names[i] = fit_name(c, t, n.cw, M_LEAD)
			elseif n.ch >= 14 then
				names[i] = fit_name(c, n.title, n.cw - (if glyphs and n.ch >= GLYPH_CELL then 4 else 0), M_NAME)
			else
				names[i] = ""
			end
		end
	end

	local function names_on(): boolean
		return view:get().zoom >= NAMES_ZOOM
	end
	local names_shown = ui.computed(names_on)
	local glyphs_shown = ui.computed(function(): boolean
		return M.node_style:get() == "provider" and view:get().zoom >= GLYPH_ZOOM
	end)

	-- The drawing: regions, combs, lines, marks, names. Run for a new
	-- layout, a state shown changing, a theme, the ease, or the view
	-- nearing what wasn't drawn: never for a pan or a zoom alone.
	local function draw(c: any, _w: number, _h: number)
		if not on_screen() then
			return
		end
		local kb0 = gcinfo()
		version:get()
		local p = palette()
		local e = ease(anim:get())
		local box = cull:get()
		local show_names = names_shown:get()
		local glyphs = glyphs_shown:get()
		status_ver:get()
		fit_names(c, glyphs)
		count("draws")
		-- (Measured from here: the count above is the tests' own.)
		kb0 = gcinfo()
		local x0, y0, x1, y1 = box.x0, box.y0, box.x1, box.y1
		-- 1. Regions.
		for i, m in layout.machines do
			local r = m.rect
			if r.x > x1 or r.y > y1 or r.x + r.w < x0 or r.y + r.h < y0 then
				continue
			end
			local fx = from_x[`dev:{m.id}`]
			if e < 1 and fx then
				local fy = from_y[`dev:{m.id}`]
				c:rect(fx + (r.x - fx) * e, fy + (r.y - fy) * e, r.w, r.h, p.region, 10)
			else
				c:rect(r.x, r.y, r.w, r.h, p.region, 10)
				c:stroke(region_paths[i], p.edge, 1)
			end
		end
		-- 2. Combs (faded in with the ease), then the lines across.
		for _, g in layout.groups do
			local r = g.rect
			if r.x > x1 or r.y > y1 or r.x + r.w < x0 or r.y + r.h < y0 or not g.lead or g.count == 0 then
				continue
			end
			local ink = if e >= 1 then hline(g.hue) else hsoft(g.hue)
			c:line(g.trunk_x, g.trunk_y0, g.trunk_x, g.trunk_y1, ink, 1.5)
			local rails = g.rails
			for k = 1, #rails, 2 do
				c:line(g.trunk_x, rails[k], rails[k + 1], rails[k], ink, 1.2)
			end
			for i = g.first, g.first + g.count - 1 do
				local n = layout.nodes[i]
				if n.cy > y1 or n.cy + n.ch < y0 then
					continue
				end
				local row = (i - g.first) // math.max(1, g.cols)
				c:line(n.x, rails[row * 2 + 1], n.x, n.y - 4, ink, 1.2)
			end
		end
		for i, ln in layout.lines do
			c:stroke(line_paths[i], if e >= 1 then hline(ln.hue) else hsoft(ln.hue), 1.2, LINE_DASH)
		end
		-- 3. Marks and names.
		local drawn, faded_n, glyph_n = 0, 0, 0
		for i, n in layout.nodes do
			if n.cx > x1 or n.cy > y1 or n.cx + n.cw < x0 - 20 or n.cy + n.ch < y0 then
				continue
			end
			drawn += 1
			local x, y = place(n, e)
			if n.lead then
				local ink = hink(n.hue)
				if n.kind == "brain" and e >= 1 then
					c:fill(brain_paths[i], if n.ghost then p.bg else ink)
					c:stroke(brain_paths[i], ink, 1.5)
				elseif n.ghost then
					c:circle(x, y, 6.5, ink)
					c:circle(x, y, 5, p.bg)
				else
					c:circle(x, y, 6.5, ink)
				end
				if show_names and e >= 1 and names[i] ~= "" then
					T_LEAD.color = if n.ghost then p.quiet else p.name
					c:text(names[i], x + 13, y, T_LEAD)
				end
				continue
			end
			local st = state_of(n.id)
			local paint, rr = p.idle, 3.2
			local bright = false
			if st == "working" then
				paint, rr, bright = p.working, 4, true
			elseif st == "waiting" then
				paint, rr, bright = p.waiting, 4, true
			elseif st == "input" or st == "approval" or st == "failed" then
				c:circle(x, y, 9, if st == "input" then p.halo_input elseif st == "approval" then p.halo_approval else p.halo_failed)
				paint, rr, bright = (p :: any)[st], 4, true
			elseif st == "done" then
				paint, rr = p.done, 3
			end
			local faded = n.fade < 10 and not bright
			if faded then
				paint = p.fade_dot[n.fade]
				faded_n += 1
			end
			-- (A glyph where there's room and the ease is done; else a dot.)
			local glyph = glyphs and e >= 1 and n.ch >= GLYPH_CELL and glyph_paths[i] ~= nil
			if glyph then
				glyph_n += 1
				c:circle(x, y, 7.5, paint)
				c:circle(x, y, if bright then 6 else 6.75, p.bg)
				local ink = if faded then p.fade_dot[n.fade] else (if bright then p.glyph else p.glyph_quiet)
				if glyph_fill[i] then
					c:fill(glyph_paths[i], ink)
				else
					c:stroke(glyph_paths[i], ink, 1.2, GLYPH_STROKE)
				end
			elseif n.kind == "thread" then
				c:circle(x, y, rr + 0.5, paint)
				c:circle(x, y, rr - 1, p.bg)
			else
				c:circle(x, y, rr, paint)
			end
			if show_names and e >= 1 and names[i] ~= "" then
				T_NAME.color = if faded then p.fade_name[n.fade] elseif bright then p.name elseif n.kind == "thread" then p.plain else p.quiet
				c:text(names[i], x + (if glyph then 12 else 8), y, T_NAME)
			end
		end
		counts.drawn, counts.faded, counts.glyphs = drawn, faded_n, glyph_n
		-- (What this draw allocated, for the tests' guard.)
		counts.draw_kb = math.max(0, gcinfo() - kb0)
	end

	-- What's lit (the node under the pointer, its line) and the dots
	-- running lines: drawn over the rest, on their own, so neither draws
	-- the rest again.
	local lit_path: any = nil
	local lit_for: string? = nil
	local function overlay(c: any, _w: number, _h: number)
		if not on_screen() then
			return
		end
		local kb0 = gcinfo()
		version:get()
		ptick:get()
		local p = palette()
		count("overlay")
		kb0 = gcinfo()
		local over = M.hover:get()
		if anim:get() < 1 then
			return
		end
		local i = over and layout.index[over]
		if i then
			local n = layout.nodes[i]
			if lit_for ~= over then
				lit_for = over
				local pts = L.path(layout, i)
				lit_path = nil
				if pts then
					local path = ui.path():move(pts[1], pts[2])
					for k = 3, #pts, 2 do
						path = path:line(pts[k], pts[k + 1])
					end
					lit_path = path
				end
			end
			if lit_path then
				c:stroke(lit_path, hink(n.hue), 2, LIT_STROKE)
			end
			c:circle(n.x, n.y, if n.lead then 9.5 else 7, p.lit)
			c:circle(n.x, n.y, if n.lead then 8.25 else 5.75, p.bg)
			c:circle(n.x, n.y, if n.lead then 6.5 else 4, if n.lead then hink(n.hue) else p.lit)
		end
		local now = ui.now()
		for _, q in pulses do
			local t = (now - q.t0) / PULSE_MS
			if t < 0 or t > 1 then
				continue
			end
			-- (Along its polyline by length.)
			local want = ease(t) * q.len
			local pts, acc = q.pts, q.acc
			local k = 1
			while k < #acc and acc[k + 1] < want do
				k += 1
			end
			local seg = (acc[k + 1] or acc[k]) - acc[k]
			local f = if seg > 0 then (want - acc[k]) / seg else 0
			local ax, ay = pts[k * 2 - 1], pts[k * 2]
			local bx, by = pts[k * 2 + 1] or ax, pts[k * 2 + 2] or ay
			local x, y = ax + (bx - ax) * f, ay + (by - ay) * f
			c:circle(x, y, 5.5, q.soft)
			c:circle(x, y, 3, q.ink)
		end
		counts.overlay_kb = math.max(0, gcinfo() - kb0)
	end

	-- Pulses: a message between two agents runs the line between them
	-- (lead to crew down the comb; a lead and its ghost by the dashed
	-- line, then down the comb), once each. The overlay redraws at 30 fps
	-- while one runs, and not at all otherwise.
	local function route(a: string, b: string): { number }?
		local ia, ib = layout.index[a], layout.index[b]
		if not ia or not ib then
			return nil
		end
		local na, nb = layout.nodes[ia], layout.nodes[ib]
		local function from_lead(lead: L.Node, crew_i: number): { number }?
			local crew = layout.nodes[crew_i]
			local comb = L.path(layout, crew_i)
			if not comb then
				return nil
			end
			local g = layout.groups[crew.group]
			local gl = g.lead and layout.nodes[g.lead]
			if gl and gl.id == lead.id then
				return comb
			end
			-- (Through its ghost: the dashed line, then the comb.)
			for _, ln in layout.lines do
				if ln.kind == "away" and layout.nodes[ln.a].id == lead.id and ln.b == g.lead then
					local out = table.clone(ln.pts)
					for k = 3, #comb do
						table.insert(out, comb[k])
					end
					return out
				end
			end
			return nil
		end
		if na.lead and not nb.lead then
			return from_lead(na, ib)
		elseif nb.lead and not na.lead then
			local pts = from_lead(nb, ia)
			if pts then
				-- (Reversed: crew to lead.)
				local out: { number } = {}
				for k = #pts - 1, 1, -2 do
					table.insert(out, pts[k])
					table.insert(out, pts[k + 1])
				end
				return out
			end
		end
		for _, ln in layout.lines do
			if ln.a == ia and ln.b == ib then
				return table.clone(ln.pts)
			end
		end
		return { na.x, na.y, nb.x, nb.y }
	end
	local pulse_run = ui.use_state(0)
	local function prune(): boolean
		local now = ui.now()
		local k = 1
		while k <= #pulses do
			if now - pulses[k].t0 > PULSE_MS then
				table.remove(pulses, k)
			else
				k += 1
			end
		end
		ptick:set(ptick:peek() + 1)
		return #pulses > 0
	end
	ui.effect(function()
		if pulse_run:get() == 0 then
			return
		end
		ui.every(33, function(): boolean?
			if not on_screen_now() then
				table.clear(pulses)
				ptick:set(ptick:peek() + 1)
				return false
			end
			if not prune() then
				return false
			end
			return nil
		end)
	end)
	local function pump()
		pulse_run:set(pulse_run:peek() + 1)
	end
	local seen: { [string]: boolean }? = nil
	ui.effect(function()
		local list = props.comms() or {}
		if seen == nil then
			seen = {}
			for _, e in list do
				(seen :: any)[e.id] = true
			end
			return
		end
		-- (Off screen: what passes is let go, never run later.)
		if not on_screen_now() then
			for _, e in list do
				(seen :: any)[e.id] = true
			end
			return
		end
		local fresh = false
		for _, e in list do
			if not (seen :: any)[e.id] then
				(seen :: any)[e.id] = true
				local pts = if motion.reduced_now() then nil else route(e.from, e.to)
				if pts and #pts >= 4 then
					local acc = { 0 }
					for k = 3, #pts, 2 do
						table.insert(acc, acc[#acc] + math.sqrt((pts[k] - pts[k - 2]) ^ 2 + (pts[k + 1] - pts[k - 1]) ^ 2))
					end
					local n = layout.nodes[layout.index[e.from]]
					local h = n and n.hue
					if not h then
						local nt = layout.nodes[layout.index[e.to]]
						h = nt and nt.hue
					end
					palette()
					table.insert(pulses, { t0 = ui.now(), pts = pts, acc = acc, len = acc[#acc], ink = hink(h), soft = hsoft(h), from = e.from, to = e.to })
					fresh = true
				end
			end
		end
		if fresh then
			pump()
		end
	end)
	M.pulses = function(): number
		return #pulses
	end

	-- The view: the drawing moved and scaled as it stands; what's drawn
	-- reaches half a screen past the field each way, drawn again only when the
	-- view comes near its edge.
	local function recull()
		local v = view:peek()
		local w, h = math.max(1, width:peek()), math.max(1, field_h:peek())
		local wx0, wy0 = -v.x / v.zoom, -v.y / v.zoom
		local wx1, wy1 = (w - v.x) / v.zoom, (h - v.y) / v.zoom
		local b = cull:peek()
		-- (Still inside what's drawn, and that not many times the view's
		-- size: nothing to draw.)
		if wx0 >= b.x0 and wy0 >= b.y0 and wx1 <= b.x1 and wy1 <= b.y1 and (b.x1 - b.x0) <= 4 * (wx1 - wx0) then
			return
		end
		local dw, dh = wx1 - wx0, wy1 - wy0
		cull:set({ x0 = wx0 - dw / 2, y0 = wy0 - dh / 2, x1 = wx1 + dw / 2, y1 = wy1 + dh / 2 })
	end
	local function set_view(x: number, y: number, zoom: number)
		view:set({ x = x, y = y, zoom = zoom })
		recull()
	end
	local function zoom_at(px: number, py: number, k: number)
		local v = view:peek()
		local zoom = math.clamp(v.zoom * k, 0.3, 3)
		local wx, wy = (px - v.x) / v.zoom, (py - v.y) / v.zoom
		set_view(px - wx * zoom, py - wy * zoom, zoom)
	end
	M.zoom_by = function(k: number)
		zoom_at(width:peek() / 2, field_h:peek() / 2, k)
	end
	M.pan_by = function(dx: number, dy: number)
		local v = view:peek()
		set_view(v.x + dx, v.y + dy, v.zoom)
	end
	ui.effect(function()
		-- (A new size: the whole field is drawn again at the start view's
		-- reach.)
		width:get()
		field_h:get()
		local v = view:peek()
		local w, h = math.max(1, width:peek()), math.max(1, field_h:peek())
		local dw, dh = w / v.zoom, h / v.zoom
		local x0, y0 = -v.x / v.zoom, -v.y / v.zoom
		cull:set({ x0 = x0 - dw / 2, y0 = y0 - dh / 2, x1 = x0 + 1.5 * dw, y1 = y0 + 1.5 * dh })
	end)

	-- What's under a point of the field: a machine's heading ("dev:<id>"),
	-- a lead (its ghost answering as the lead), a thread.
	local function hit(px: number, py: number): string?
		local v = view:peek()
		local x, y = (px - v.x) / v.zoom, (py - v.y) / v.zoom
		for _, m in layout.machines do
			local r = m.rect
			if x >= r.x and x < r.x + r.w and y >= r.y and y < r.y + L.HEAD then
				return `dev:{m.id}`
			end
		end
		local i = L.cell_at(layout, x, y)
		if not i then
			return nil
		end
		return layout.nodes[i].id
	end
	local function real(id: string): string
		local r = string.match(id, "^ghost:(.-)@")
		return r or id
	end
	-- A node's box on screen (its mark and name): x, y, w, h.
	local function screen_box(id: string): (number, number, number, number)
		local v = view:peek()
		local i = layout.index[id]
		if i then
			local n = layout.nodes[i]
			local bx, by = n.x - 8, n.cy
			local bw = (n.cx + n.cw) - bx
			return bx * v.zoom + v.x, by * v.zoom + v.y, bw * v.zoom, n.ch * v.zoom
		end
		local dev = string.match(id, "^dev:(.*)$")
		for _, m in layout.machines do
			if m.id == dev then
				return m.rect.x * v.zoom + v.x, m.rect.y * v.zoom + v.y, m.rect.w * v.zoom, L.HEAD * v.zoom
			end
		end
		return -1000, -1000, 0, 0
	end
	local function open(id: string)
		id = real(id)
		if string.sub(id, 1, 4) == "dev:" then
			return
		end
		local i = layout.index[id]
		local n = i and layout.nodes[i]
		if not n then
			return
		end
		if n.lead then
			props.on_open_orchestrator(id)
		else
			props.on_open_chat(id)
		end
	end
	local function ref_of(id: string): any
		id = real(id)
		local dev = string.match(id, "^dev:(.*)$")
		if dev then
			for _, d in props.world().devices or {} do
				if d.id == dev then
					return Ref.device(d)
				end
			end
			return nil
		end
		local i = layout.index[id]
		return kit.ref_of(props.world, id, if i then layout.nodes[i].title else nil)
	end

	-- The pointer: a press and a drag pans; a press let go where it was
	-- opens what's there; resting lights it.
	local press: any = nil
	local grip_hot = ui.use_state(false)
	local in_field = false
	local function on_press(x: number, y: number)
		press = { id = hit(x, y), x = x, y = y, moved = false, view = view:peek() }
	end
	local function on_move(x: number, y: number)
		if press then
			local dx, dy = x - press.x, y - press.y
			if not press.moved and dx * dx + dy * dy < 16 then
				return
			end
			press.moved = true
			set_view(press.view.x + dx, press.view.y + dy, press.view.zoom)
			return
		end
		local id = hit(x, y)
		if id ~= M.hover:peek() then
			M.hover:set(id)
		end
	end
	local function on_release(x: number, y: number)
		local p = press
		press = nil
		if p and not p.moved then
			local id = hit(x, y)
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

	-- The grip beside the node under the pointer: dragged, a reference to
	-- it (ds/orch_ref) for Capture's composer. (A machine's heading drags
	-- its own.)
	local grip_for = ui.use_state(nil :: string?)
	ui.effect(function()
		local id = M.hover:get()
		if id and id ~= grip_for:peek() and string.sub(id, 1, 4) ~= "dev:" then
			grip_for:set(id)
		end
	end)
	local function grip_at(): (number, number)
		view:get()
		version:get()
		local id = grip_for:get()
		if not id then
			return -1000, -1000
		end
		local i = layout.index[id]
		if not i then
			return -1000, -1000
		end
		local n = layout.nodes[i]
		local v = view:get()
		local fx = n.cx + math.min(n.cw, (n.title and #n.title or 0) * 7 + 8)
		return math.min(fx * v.zoom + v.x + 4, width:get() - 24), n.y * v.zoom + v.y
	end
	local grip = ui.div {
		id = "orch-graph2-grip",
		class = "orch-graph2-grip",
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

	-- The card of what the pointer rests on: anchored on its box (one
	-- anchor, moved to whatever is lit), from ds/hover_card.
	local function card_for(): Node
		local function id(): string?
			local h = M.hover:get()
			return if h then real(h) else nil
		end
		local function c(): any
			local x = id()
			return if x then props.chat(x) or {} else {}
		end
		local function machine(): any
			local x = id()
			local dev = x and string.match(x, "^dev:(.*)$")
			if not dev then
				return nil
			end
			for _, d in props.world().devices or {} do
				if d.id == dev then
					return d
				end
			end
			return nil
		end
		local function node(): L.Node?
			version:get()
			local x = id()
			local i = x and layout.index[x]
			return if i then layout.nodes[i] else nil
		end
		local function st(): string
			local x = id()
			return if x and not machine() then state_of(x) else "idle"
		end
		local function title(): string
			local d = machine()
			if d then
				return d.name or ""
			end
			local n = node()
			return if n then n.title else ""
		end
		local function doing(): string
			local d = machine()
			if d then
				local l = ((props.world().loads or {}) :: any)[d.id]
				return if l and l.cpu then locale.s("ds.orch.map.cpu", { percent = math.floor(l.cpu * 100 + 0.5) }) else ""
			end
			local s = st()
			local a = c().activity
			if s == "working" and a then
				return locale.s(`ds.orch.activity.{a.kind}`)
			end
			return if s ~= "idle" and status.KINDS[s :: any] then status.KINDS[s :: any].long else ""
		end
		local function running(): string
			local a = c().activity
			return if st() == "working" and a then a.label or "" else ""
		end
		local function device_name(): string
			local n = node()
			if not n then
				return ""
			end
			local m = layout.machines[n.machine]
			return if m then m.name else ""
		end
		return HoverCard.Card {
			class = "orch-graph2-card",
			w = 248,
			p = "$space.3",
			gap = "$space.1",
			ui.row {
				gap = "$space.2",
				items = "center",
				ui.show(function(): boolean
					local n = node()
					return n ~= nil and not n.lead and st() ~= "idle"
				end, status.StatusIcon { kind = function(): any
					return st()
				end, size = 14 }),
				ui.text {
					title,
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
			ui.show(function(): boolean
				return running() ~= ""
			end, ui.text {
				running,
				literal = true,
				truncate = true,
				w_full = true,
				font = "$font.mono",
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			}),
			ui.show(function(): boolean
				return machine() == nil
			end, ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				ui.div { color = "$text.tertiary", shrink = 0, Icon { name = "monitor", size = 12 } },
				ui.text { device_name, text_size = "$font.size.xs", color = "$text.tertiary", truncate = true, min_w = 0 },
				ui.text {
					-- (Its provider and model.)
					function(): string
						local chat = c()
						local pv = Scene.provider(chat)
						local who = if pv == "other" then tostring(chat.agent or chat.harness or "") else locale.s(`ds.orch.map.provider.{pv}`)
						local model = chat.model or ""
						return if model ~= "" and who ~= "" then `{who}  {model}` else who .. model
					end,
					grow = 1,
					align = "end",
					truncate = true,
					min_w = 0,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			}),
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
	local function hover_box(): (number, number, number, number)
		view:get()
		version:get()
		local id = M.hover:get()
		if not id then
			return -1000, -1000, 0, 0
		end
		return screen_box(id)
	end
	-- (The anchor takes the pointer over what's lit, so it hands its
	-- presses and moves to the field's own, in the field's points.)
	local anchor_at = { x = 0, y = 0 }
	local anchor = HoverCard.Anchored {
		class = "orch-graph2-anchor",
		w = 248,
		delay = 350,
		placement = { side = "bottom", align = "start", offset = 6 },
		anchor = ui.div {
			class = "orch-graph2-anchor-box",
			w = function(): number
				local _, _, w = hover_box()
				return math.max(1, w)
			end,
			h = function(): number
				local _, _, _, h = hover_box()
				return math.max(1, h)
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
			return card_for()
		end,
	}

	-- A node's marker: where it is, for assistive technology and tests
	-- (no pointer of its own). In a layer the pan moves whole.
	local items = ui.computed(function(): { any }
		version:get()
		local out = {}
		for _, n in layout.nodes do
			table.insert(out, { id = n.id, kind = n.kind, lead = n.lead, ghost = n.ghost })
		end
		return out
	end)
	local function marker(item: any): Node
		local st = if not item.ghost
			then kit.held(function(): any
				return props.chat(item.id)
			end)
			else nil
		if st then
			local held = st
			ui.effect(function()
				if not on_screen() then
					return
				end
				local now = held()
				if status_by[item.id] ~= now then
					status_by[item.id] = now
					status_ver:set(status_ver:peek() + 1)
				end
			end)
			ui.on_cleanup(function()
				status_by[item.id] = nil
			end)
		end
		local function n(): L.Node?
			version:get()
			local i = layout.index[item.id]
			return if i then layout.nodes[i] else nil
		end
		local kind = if item.ghost then "ghost" elseif item.lead then (if item.kind == "brain" then "brain" else "orchestrator") else "thread"
		return ui.div {
			class = `orch-graph2-node orch-graph2-{kind}`,
			label = function(): string
				local x = n()
				local t = if x then x.title else ""
				local s = if st then st() else "idle"
				if s ~= "idle" and status.KINDS[s :: any] then
					return `{t}, {status.KINDS[s :: any].long}`
				end
				return t
			end,
			absolute = true,
			pointer_events = "none",
			left = function(): number
				local x = n()
				return if x then (x.x - 5) * view:get().zoom else -1000
			end,
			top = function(): number
				local x = n()
				return if x then (x.y - 5) * view:get().zoom else -1000
			end,
			w = function(): number
				return 10 * view:get().zoom
			end,
			h = function(): number
				return 10 * view:get().zoom
			end,
		}
	end

	-- Each machine's heading over its region: its mark, name, platform and
	-- CPU (ds/orch_machine_head), the same as in the Map's other views.
	local machines = ui.computed(function(): { any }
		version:get()
		local out = {}
		for _, m in layout.machines do
			table.insert(out, { id = m.id })
		end
		return out
	end)
	local function heading(item: any): Node
		local function m(): any
			version:get()
			for _, x in layout.machines do
				if x.id == item.id then
					return x
				end
			end
			return nil
		end
		local function device(): any
			for _, d in props.world().devices or {} do
				if d.id == item.id then
					return d
				end
			end
			return { id = item.id, name = (m() or {}).name or "" }
		end
		return ui.div {
			class = "orch-graph2-machine",
			label = function(): string
				return device().name or ""
			end,
			absolute = true,
			pointer_events = "none",
			left = function(): number
				local x = m()
				return if x then (x.rect.x + 12) * view:get().zoom else -1000
			end,
			top = function(): number
				local x = m()
				return if x then (x.rect.y + 3) * view:get().zoom else -1000
			end,
			w = function(): number
				local x = m()
				return if x then math.max(40, (x.rect.w - 24) * view:get().zoom) else 0
			end,
			MachineHead.MachineHead {
				device = device,
				here = function(): boolean
					return item.id == props.world().here
				end,
				load = function(): any
					return ((props.world().loads or {}) :: any)[item.id]
				end,
				load_at_end = true,
			},
		}
	end

	local function canvas_view(): any
		return view:get()
	end

	return ui.column {
		class = "orch-map-graph2",
		w_full = true,
		relative = true,
		on_resize = function(size: { w: number, h: number })
			if math.abs(size.w - width:peek()) >= 1 then
				width:set(size.w)
			end
		end,
		kit.MenuField {
			menu = props.menu,
			register = props.register,
			key = "graph2",
			hit = function(x: number, y: number): string?
				local id = hit(x, y)
				return if id then real(id) else nil
			end,
			center = function(id: string): (number?, number?)
				local x, y, w, h = screen_box(id)
				if x < -999 then
					return nil, nil
				end
				return x + math.min(w, 24) / 2, y + h / 2
			end,
			child = ui.div {
				class = "orch-graph2-field",
				id = "orch-graph2-field",
				relative = true,
				w_full = true,
				h = function(): number
					return field_h:get()
				end,
				overflow = "hidden",
				rounded = "$radius.lg",
				on_visible = function(visible: boolean)
					if visible ~= in_view:peek() then
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
				ui.canvas {
					class = "orch-graph2-canvas",
					absolute = true,
					left = 0,
					right = 0,
					top = 0,
					bottom = 0,
					pointer_events = "none",
					label = function(): string
						return locale.s("ds.orch.map.variant.graph2")
					end,
					view = canvas_view,
					draw = draw,
				},
				ui.canvas {
					class = "orch-graph2-overlay",
					absolute = true,
					left = 0,
					right = 0,
					top = 0,
					bottom = 0,
					pointer_events = "none",
					view = canvas_view,
					draw = overlay,
				},
				-- (Markers and headings in a layer the pan moves whole; only on
				-- screen.)
				ui.show(on_screen, ui.div {
					class = "orch-graph2-layer",
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
					ui.each(machines, heading, { key = function(item: any): string
						return item.id
					end, absolute = true, left = 0, top = 0, pointer_events = "none" }),
				}),
				ui.div {
					class = "orch-graph2-anchor-at",
					absolute = true,
					left = function(): number
						local x = hover_box()
						anchor_at.x = x
						return x
					end,
					top = function(): number
						local _, y = hover_box()
						anchor_at.y = y
						return y
					end,
					ui.show(function(): boolean
						local id = M.hover:get()
						return id ~= nil and string.sub(id, 1, 4) ~= "dev:"
					end, anchor),
				},
				ui.row {
					absolute = true,
					right = 8,
					bottom = 8,
					gap = "$space.1",
					IconButton {
						id = "orch-graph2-fit",
						icon = "expand-arrows",
						label = locale.t("ds.orch.map.fit"),
						size = "sm",
						tone = "quiet",
						on_click = function()
							set_view(0, 0, 1)
						end,
					},
				},
			},
		},
		ui.show(function(): boolean
			local id = M.hover:get()
			return (id ~= nil and string.sub(id, 1, 4) ~= "dev:") or grip_hot:get()
		end, grip),
	}
end)

return M

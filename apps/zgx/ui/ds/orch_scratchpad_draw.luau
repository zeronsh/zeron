-- The Scratchpad's drawing: shapes on a zgx canvas, drawn with the
-- pointer. Tools: select (click, Shift-click, or a marquee; move what's
-- chosen, resize a box, an ellipse or a text by its corners, bend an arrow
-- by its middle, delete), pen (freehand, smoothed as it is drawn and
-- simplified as it ends), rectangle, ellipse, arrow (straight, or with one
-- elbow: its `points` are start, elbow?, end) and text. A double click on
-- a box, an ellipse, an arrow or a text edits its words. Colours are theme
-- tokens. Undo and redo are this session's, over the engine's history.
-- As the Map's web: a scroll over it is the page's, zoom with ⌘/Ctrl and a
-- scroll or a pinch. Pressed into, it takes the scroll (a plain one pans)
-- until Escape, a press elsewhere or the pointer leaving lets it go. The view is this one's,
-- never saved.
--
--   Draw.Board { id?, shapes = channel, h = number? | grow }  -- the board
--   Draw.channel()        the drawing's autosave (ds/orch_scratchpad_sync)
--   Draw.ink(token)       a shape's colour token as the theme's
--   Draw.draw_shape(c, shape)        one shape on a canvas, in its units
--   Draw.simplify(points, epsilon)   Ramer–Douglas–Peucker
--   Draw.fit(shapes, w, h)           the view that shows them all
--
-- Keys, while the board has focus (never while a field does): V select,
-- P pen, R rectangle, O ellipse, A arrow, T text, Delete or Backspace
-- deletes what's chosen, ⌘Z undo, ⇧⌘Z or Ctrl+Y redo, Escape lets go,
-- ⇧1 fits the drawing.

local IconButton = require("@self/icon_button").IconButton
local Sync = require("@self/orch_scratchpad_sync")
local locale = require("@self/locale")
local material = require("@self/material")

local M = {}

export type Point = { x: number, y: number }
export type Shape = {
	id: string,
	kind: string,
	x: number,
	y: number,
	w: number,
	h: number,
	points: { Point },
	text: string,
	color: string,
	stroke: number?,
	z: number,
	[string]: any,
}
type View = { x: number, y: number, zoom: number }

-- The colours a shape may take (tokens, as the wire keeps them), in the
-- palette's order.
M.COLORS = { "fg", "accent", "red", "orange", "green", "blue", "purple" }
local INK: { [string]: string } = {
	fg = "$text.primary",
	muted = "$text.tertiary",
	accent = "$accent",
	red = "$palette.red.500",
	orange = "$palette.orange.500",
	yellow = "$palette.amber.500",
	green = "$palette.green.500",
	blue = "$palette.blue.500",
	purple = "$palette.violet.500",
}
function M.ink(token: string?): string
	return INK[token or "fg"] or INK.fg
end

M.TOOLS = {
	{ id = "select", icon = "cursor", key = "v" },
	{ id = "pen", icon = "pen", key = "p" },
	{ id = "rect", icon = "shape-rect", key = "r" },
	{ id = "ellipse", icon = "shape-ellipse", key = "o" },
	{ id = "arrow", icon = "arrow-up-right", key = "a" },
	{ id = "text", icon = "shape-text", key = "t" },
}
local TOOL_OF_KEY: { [string]: string } = {}
for _, t in M.TOOLS do
	TOOL_OF_KEY[t.key] = t.id
end

-- Canvas units: a line's width and a label's size as drawn at zoom 1.
local LINE = 2
local LABEL = 14
-- A new box or ellipse made by a click (no drag), and the least a drag
-- makes one.
local DEFAULT_W, DEFAULT_H = 140, 72
local MIN_SIDE = 6
-- How near a shape or a handle a press counts as on it, in points.
local REACH = 6
local HANDLE = 8

-- What a shape is, compared: its fields in one string (the autosave's
-- signature, so equal drawings are one).
local function num(v: number?): string
	return string.format("%.2f", v or 0)
end
local function sig_of(list: { Shape }): string
	local parts = table.create(#list)
	for i, s in list do
		local pts = table.create(#(s.points or {}))
		for j, p in s.points or {} do
			pts[j] = `{num(p.x)},{num(p.y)}`
		end
		parts[i] = table.concat({ s.id, s.kind, num(s.x), num(s.y), num(s.w), num(s.h), table.concat(pts, ";"), s.text or "", s.color or "", num(s.stroke), tostring(s.z or 0) }, "|")
	end
	return table.concat(parts, "\n")
end
M.sig = sig_of

function M.channel(): Sync.Channel
	return Sync.channel("shapes", {}, sig_of)
end

local tool = ui.state("ds_scratchpad_tool", "select")
local color = ui.state("ds_scratchpad_color", "fg")
local selected = ui.state("ds_scratchpad_selected", {} :: { [string]: boolean })
M.tool = tool
M.color = color
M.selected = selected

-- This session's steps: each a drawing as it was before a change.
local undo_stack: { { Shape } } = {}
local redo_stack: { { Shape } } = {}
-- (Followed by the buttons.)
local steps = ui.state("ds_scratchpad_steps", { undo = 0, redo = 0 })
local function counted()
	steps:set({ undo = #undo_stack, redo = #redo_stack })
end

-- Points simplified: Ramer–Douglas–Peucker, the line kept within
-- `epsilon` of every point dropped.
function M.simplify(points: { Point }, epsilon: number): { Point }
	local n = #points
	if n <= 2 then
		return table.clone(points)
	end
	local keep = table.create(n, false)
	keep[1], keep[n] = true, true
	local stack = { { 1, n } }
	while #stack > 0 do
		local span = table.remove(stack) :: { number }
		local a, b = span[1], span[2]
		local pa, pb = points[a], points[b]
		local dx, dy = pb.x - pa.x, pb.y - pa.y
		local len = math.sqrt(dx * dx + dy * dy)
		local far, at = 0, 0
		for i = a + 1, b - 1 do
			local p = points[i]
			local d = if len < 1e-9
				then math.sqrt((p.x - pa.x) ^ 2 + (p.y - pa.y) ^ 2)
				else math.abs(dy * p.x - dx * p.y + pb.x * pa.y - pb.y * pa.x) / len
			if d > far then
				far, at = d, i
			end
		end
		if far > epsilon and at > 0 then
			keep[at] = true
			table.insert(stack, { a, at })
			table.insert(stack, { at, b })
		end
	end
	local out = {}
	for i, p in points do
		if keep[i] then
			table.insert(out, { x = p.x, y = p.y })
		end
	end
	return out
end

-- A shape's box, its corners in order (a line's around its points).
type Box = { x: number, y: number, w: number, h: number }
local function lo(a: number, b: number): number
	return if a < b then a else b
end
local function hi(a: number, b: number): number
	return if a > b then a else b
end
local function box_of(s: Shape): Box
	if s.kind == "stroke" or s.kind == "arrow" then
		local pts = s.points or {}
		if #pts == 0 then
			return { x = s.x or 0, y = s.y or 0, w = 0, h = 0 }
		end
		local x0, y0, x1, y1 = pts[1].x, pts[1].y, pts[1].x, pts[1].y
		for _, p in pts do
			x0 = lo(x0, p.x)
			y0 = lo(y0, p.y)
			x1 = hi(x1, p.x)
			y1 = hi(y1, p.y)
		end
		return { x = x0, y = y0, w = x1 - x0, h = y1 - y0 }
	end
	local x, y, w, h = s.x or 0, s.y or 0, s.w or 0, s.h or 0
	if w < 0 then
		x, w = x + w, -w
	end
	if h < 0 then
		y, h = y + h, -h
	end
	return { x = x, y = y, w = w, h = h }
end
M.box_of = box_of

function M.fit(list: { Shape }, w: number, h: number): View
	if #list == 0 or w <= 0 or h <= 0 then
		return { x = 0, y = 0, zoom = 1 }
	end
	local first = box_of(list[1])
	local x0, y0, x1, y1 = first.x, first.y, first.x + first.w, first.y + first.h
	for _, s in list do
		local b = box_of(s)
		x0 = lo(x0, b.x)
		y0 = lo(y0, b.y)
		x1 = hi(x1, b.x + b.w)
		y1 = hi(y1, b.y + b.h)
	end
	-- (Room for the words beside a line, past its box.)
	local pad = math.min(48, math.min(w, h) / 6)
	local bw, bh = math.max(x1 - x0, 1), math.max(y1 - y0, 1)
	local zoom = math.clamp(math.min((w - pad * 2) / bw, (h - pad * 2) / bh), 0.1, 1.25)
	return { x = (w - bw * zoom) / 2 - x0 * zoom, y = (h - bh * zoom) / 2 - y0 * zoom, zoom = zoom }
end

local function by_z(list: { Shape }): { Shape }
	local out = table.clone(list)
	table.sort(out, function(a: Shape, b: Shape): boolean
		if (a.z or 0) ~= (b.z or 0) then
			return (a.z or 0) < (b.z or 0)
		end
		return a.id < b.id
	end)
	return out
end

-- A polyline's distance from `p`.
local function seg_dist(p: Point, a: Point, b: Point): number
	local dx, dy = b.x - a.x, b.y - a.y
	local l2 = dx * dx + dy * dy
	local t = if l2 < 1e-9 then 0 else math.clamp(((p.x - a.x) * dx + (p.y - a.y) * dy) / l2, 0, 1)
	local qx, qy = a.x + t * dx, a.y + t * dy
	return math.sqrt((p.x - qx) ^ 2 + (p.y - qy) ^ 2)
end

local function hit(s: Shape, p: Point, tol: number): boolean
	if s.kind == "stroke" or s.kind == "arrow" then
		local pts = s.points or {}
		local reach = tol + (s.stroke or LINE) / 2
		if #pts == 1 then
			return seg_dist(p, pts[1], pts[1]) <= reach
		end
		for i = 2, #pts do
			if seg_dist(p, pts[i - 1], pts[i]) <= reach then
				return true
			end
		end
		return false
	end
	local b = box_of(s)
	if s.kind == "ellipse" then
		local rx, ry = b.w / 2 + tol, b.h / 2 + tol
		local cx, cy = b.x + b.w / 2, b.y + b.h / 2
		return ((p.x - cx) / rx) ^ 2 + ((p.y - cy) / ry) ^ 2 <= 1
	end
	return p.x >= b.x - tol and p.x <= b.x + b.w + tol and p.y >= b.y - tol and p.y <= b.y + b.h + tol
end

-- The topmost shape under `p`.
local function shape_at(list: { Shape }, p: Point, tol: number): Shape?
	local sorted = by_z(list)
	for i = #sorted, 1, -1 do
		if hit(sorted[i], p, tol) then
			return sorted[i]
		end
	end
	return nil
end

local function boxy(s: Shape): boolean
	return s.kind == "rect" or s.kind == "ellipse" or s.kind == "text"
end

-- The middle of an arrow (its elbow, or halfway along it).
local function middle_of(s: Shape): Point
	local pts = s.points or {}
	if #pts >= 3 then
		return pts[2]
	end
	local a, b = pts[1] or { x = 0, y = 0 }, pts[#pts] or { x = 0, y = 0 }
	return { x = (a.x + b.x) / 2, y = (a.y + b.y) / 2 }
end

-- A shape's grips, in canvas units: a box's corners, an arrow's ends and
-- middle.
local function grips_of(s: Shape): { { id: string, x: number, y: number } }
	if boxy(s) then
		local b = box_of(s)
		return {
			{ id = "nw", x = b.x, y = b.y },
			{ id = "ne", x = b.x + b.w, y = b.y },
			{ id = "se", x = b.x + b.w, y = b.y + b.h },
			{ id = "sw", x = b.x, y = b.y + b.h },
		}
	elseif s.kind == "arrow" then
		local pts = s.points or {}
		if #pts < 2 then
			return {}
		end
		local m = middle_of(s)
		return { { id = "start", x = pts[1].x, y = pts[1].y }, { id = "mid", x = m.x, y = m.y }, { id = "end", x = pts[#pts].x, y = pts[#pts].y } }
	end
	return {}
end

local serial = 0
local function new_id(): string
	serial += 1
	return `sh-{math.floor(ui.now())}-{serial}`
end

local function top_z(list: { Shape }): number
	local z = 0
	for _, s in list do
		z = math.max(z, s.z or 0)
	end
	return z
end

local function copy(s: Shape): Shape
	local c = table.clone(s)
	local pts = {}
	for i, p in s.points or {} do
		pts[i] = { x = p.x, y = p.y }
	end
	c.points = pts
	return c
end

local function moved(s: Shape, dx: number, dy: number): Shape
	local c = copy(s)
	if s.kind == "stroke" or s.kind == "arrow" then
		for _, p in c.points do
			p.x += dx
			p.y += dy
		end
	else
		c.x = (s.x or 0) + dx
		c.y = (s.y or 0) + dy
	end
	return c
end

-- A box resized by a corner dragged to `p`, the opposite one held.
local function resized(s: Shape, grip: string, p: Point): Shape
	local b = box_of(s)
	local fx = if grip == "nw" or grip == "sw" then b.x + b.w else b.x
	local fy = if grip == "nw" or grip == "ne" then b.y + b.h else b.y
	local c = copy(s)
	c.x, c.y = math.min(fx, p.x), math.min(fy, p.y)
	c.w, c.h = math.max(math.abs(p.x - fx), MIN_SIDE), math.max(math.abs(p.y - fy), MIN_SIDE)
	return c
end

local function regripped(s: Shape, grip: string, p: Point): Shape
	if boxy(s) then
		return resized(s, grip, p)
	end
	local c = copy(s)
	local pts = c.points
	if grip == "start" then
		pts[1] = { x = p.x, y = p.y }
	elseif grip == "end" then
		pts[#pts] = { x = p.x, y = p.y }
	elseif grip == "mid" then
		-- (The elbow, made as it's first pulled.)
		if #pts >= 3 then
			pts[2] = { x = p.x, y = p.y }
		else
			c.points = { pts[1], { x = p.x, y = p.y }, pts[#pts] }
		end
	end
	return c
end

-- A line through points, smoothed: quadratic curves through their middles.
local function smooth_path(pts: { Point }): any
	local path = ui.path():move(pts[1].x, pts[1].y)
	if #pts == 2 then
		return path:line(pts[2].x, pts[2].y)
	end
	for i = 2, #pts - 1 do
		local p, q = pts[i], pts[i + 1]
		path = path:quad(p.x, p.y, (p.x + q.x) / 2, (p.y + q.y) / 2)
	end
	local last = pts[#pts]
	return path:line(last.x, last.y)
end

-- A label on the canvas: its lines centred on (`cx`, `cy`).
local function label(c: any, text: string, cx: number, cy: number, ink: string)
	local lines = string.split(text, "\n")
	local lh = LABEL * 1.3
	local top = cy - (#lines * lh) / 2 + lh / 2
	for i, line in lines do
		local y = top + (i - 1) * lh
		c:text(line, cx, y, { size = LABEL, color = ink, align = "center", baseline = "middle" })
	end
end

-- One shape, in canvas units.
function M.draw_shape(c: any, s: Shape, hide_text: boolean?)
	local ink = M.ink(s.color)
	local width = s.stroke or LINE
	local opts = { cap = "round", join = "round" }
	local text = if hide_text then "" else (s.text or "")
	if s.kind == "stroke" then
		local pts = s.points or {}
		if #pts == 1 then
			c:circle(pts[1].x, pts[1].y, width / 2, ink)
		elseif #pts >= 2 then
			c:stroke(smooth_path(pts), ink, width, opts)
		end
	elseif s.kind == "rect" or s.kind == "ellipse" then
		local b = box_of(s)
		local path = if s.kind == "rect" then ui.path():rect(b.x, b.y, b.w, b.h, math.min(8, b.w / 4, b.h / 4)) else ui.path():ellipse(b.x, b.y, b.w, b.h)
		c:stroke(path, ink, width, opts)
		if text ~= "" then
			label(c, text, b.x + b.w / 2, b.y + b.h / 2, ink)
		end
	elseif s.kind == "arrow" then
		local pts = s.points or {}
		if #pts >= 2 then
			local path = ui.path():move(pts[1].x, pts[1].y)
			for i = 2, #pts do
				path = path:line(pts[i].x, pts[i].y)
			end
			c:stroke(path, ink, width, opts)
			-- Its head, along the last stretch.
			local a, b = pts[#pts - 1], pts[#pts]
			local angle = math.atan2(b.y - a.y, b.x - a.x)
			local len = 10 + width * 2
			local head = ui.path()
				:move(b.x - len * math.cos(angle - 0.45), b.y - len * math.sin(angle - 0.45))
				:line(b.x, b.y)
				:line(b.x - len * math.cos(angle + 0.45), b.y - len * math.sin(angle + 0.45))
			c:stroke(head, ink, width, opts)
			if text ~= "" then
				-- Its words beside it: off its elbow, away from the line
				-- between its ends; over a straight one.
				local m = middle_of(s)
				local first, last = pts[1], pts[#pts]
				local nx, ny = 0, -1
				if #pts >= 3 then
					local dx, dy = m.x - (first.x + last.x) / 2, m.y - (first.y + last.y) / 2
					local len = math.sqrt(dx * dx + dy * dy)
					if len > 1e-6 then
						nx, ny = dx / len, dy / len
					end
				end
				label(c, text, m.x + nx * LABEL, m.y + ny * LABEL, ink)
			end
		end
	elseif s.kind == "text" then
		local lines = string.split(text, "\n")
		for i, line in lines do
			c:text(line, s.x or 0, (s.y or 0) + (i - 1) * LABEL * 1.3, { size = LABEL, color = ink, baseline = "top" })
		end
	end
end

-- A text's box as it's drawn (for selecting and resizing it).
local function text_box(text: string): (number, number)
	local lines = string.split(text, "\n")
	local longest = 0
	for _, l in lines do
		longest = math.max(longest, utf8.len(l) or #l)
	end
	return math.max(longest * LABEL * 0.56, LABEL), #lines * LABEL * 1.3
end

local CURSOR = ui.variants {
	base = { cursor = "crosshair" },
	variants = {
		tool = {
			select = { cursor = "default" },
			pen = { cursor = "crosshair" },
			rect = { cursor = "crosshair" },
			ellipse = { cursor = "crosshair" },
			arrow = { cursor = "crosshair" },
			text = { cursor = "text" },
		},
	},
	defaults = { tool = "select" },
}

export type BoardProps = {
	id: string?,
	h: any?,
	grow: number?,
	shapes: Sync.Channel,
}

-- The board's fill (the cards' quiet one).
local function plate(): () -> string
	return material.pick("$zeron.settings.note", "$zeron.settings.card")
end

-- Places a test drives the board by (`ds_scratchpad_probe`): a grid of
-- small marks, every tenth of its width and height, `#scratchpad-at-<i>-<j>`.
local probe = ui.state("ds_scratchpad_probe", false)

M.Board = ui.component(function(props: BoardProps): Node
	local ch = props.shapes
	local size = ui.use_state({ w = 0, h = 0 })
	local view = ui.use_state({ x = 0, y = 0, zoom = 1 } :: View)
	-- (Panned or zoomed by the person: no longer fitted to the drawing.)
	local touched = ui.use_state(false)
	-- Pressed into: a plain wheel pans it, until Escape or a press elsewhere.
	-- Otherwise a plain wheel over it is the page's (scrolling past it).
	local active = ui.use_state(false)
	local board_el: any = nil
	-- What a gesture draws before it's a shape (its own canvas, in screen
	-- points), and the marquee.
	local draft = ui.use_state(nil :: Shape?)
	local marquee = ui.use_state(nil :: Box?)
	-- A text being written: where (canvas units), its words, and the shape
	-- whose words they are (nil for a new text).
	local writing = ui.use_state(nil :: { x: number, y: number, id: string?, text: string }?)
	local words = ui.use_state("")
	local fill = plate()

	local gesture: { [string]: any }? = nil
	ch.busy = function(): boolean
		return gesture ~= nil
	end

	local function shapes(): { Shape }
		return ch.value:peek()
	end
	local function fit()
		local s = size:peek()
		view:set(M.fit(shapes(), s.w, s.h))
	end
	-- Fitted as it's sized and as the drawing comes, until the person moves it.
	ui.effect(function()
		local s = size:get()
		if touched:get() or s.w <= 0 then
			return
		end
		ch.adopted:get()
		fit()
	end)

	local function to_canvas(lx: number, ly: number): Point
		local v = view:peek()
		return { x = (lx - v.x) / v.zoom, y = (ly - v.y) / v.zoom }
	end

	local function change(list: { Shape })
		table.insert(undo_stack, shapes())
		if #undo_stack > 200 then
			table.remove(undo_stack, 1)
		end
		redo_stack = {}
		counted()
		ch.edit(list)
	end
	local function only(ids: { string })
		local sel = {}
		for _, id in ids do
			sel[id] = true
		end
		selected:set(sel)
	end
	local function chosen(): { Shape }
		local sel = selected:peek()
		local out = {}
		for _, s in shapes() do
			if sel[s.id] then
				table.insert(out, s)
			end
		end
		return out
	end
	local function add(s: Shape)
		local list = table.clone(shapes())
		s.z = top_z(list) + 1
		table.insert(list, s)
		change(list)
	end

	local function delete()
		local sel = selected:peek()
		if next(sel) == nil then
			return
		end
		local list = {}
		for _, s in shapes() do
			if not sel[s.id] then
				table.insert(list, s)
			end
		end
		change(list)
		selected:set({})
	end
	local function step(from: { { Shape } }, to: { { Shape } })
		local back = table.remove(from)
		if not back then
			return
		end
		table.insert(to, shapes())
		counted()
		ch.edit(back)
		-- (What's chosen and gone with it, let go.)
		local sel, have = selected:peek(), {}
		for _, s in back do
			have[s.id] = true
		end
		local kept = {}
		for id in sel do
			if have[id] then
				kept[id] = true
			end
		end
		selected:set(kept)
	end
	local function undo()
		step(undo_stack, redo_stack)
	end
	local function redo()
		step(redo_stack, undo_stack)
	end
	M.undo, M.redo = undo, redo

	-- A text's words written: a new text made, or a shape's words changed.
	local function commit_words()
		local w = writing:peek()
		if not w then
			return
		end
		writing:set(nil)
		local text = string.match(words:peek(), "^%s*(.-)%s*$") or ""
		if w.id then
			local list = {}
			local changed = false
			for _, s in shapes() do
				if s.id == w.id and (s.text or "") ~= text then
					s = copy(s)
					s.text = text
					if s.kind == "text" then
						s.w, s.h = text_box(text)
					end
					changed = true
				end
				table.insert(list, s)
			end
			if changed then
				change(list)
			end
		elseif text ~= "" then
			local tw, th = text_box(text)
			local id = new_id()
			add({ id = id, kind = "text", x = w.x, y = w.y, w = tw, h = th, points = {}, text = text, color = color:peek(), z = 0 })
			only({ id })
		end
	end
	local function write_at(p: Point, s: Shape?)
		commit_words()
		words:set(if s then s.text or "" else "")
		if s then
			local b = box_of(s)
			local at = if s.kind == "arrow" then middle_of(s) else { x = b.x, y = b.y }
			writing:set({ x = at.x, y = at.y, id = s.id, text = s.text or "" })
		else
			writing:set({ x = p.x, y = p.y, text = "" })
		end
	end

	local function grip_at(lx: number, ly: number): (Shape?, string?)
		local list = chosen()
		if #list ~= 1 then
			return nil, nil
		end
		local s = list[1]
		local v = view:peek()
		for _, g in grips_of(s) do
			local gx, gy = g.x * v.zoom + v.x, g.y * v.zoom + v.y
			if math.abs(lx - gx) <= HANDLE and math.abs(ly - gy) <= HANDLE then
				return s, g.id
			end
		end
		return nil, nil
	end

	-- Where the words being written stand, in screen points.
	local WORDS_W, WORDS_H = 220, 32
	local function in_words(lx: number, ly: number): boolean
		local w, v = writing:peek(), view:peek()
		if not w then
			return false
		end
		local x, y = w.x * v.zoom + v.x - 6, w.y * v.zoom + v.y - 6
		return lx >= x and lx <= x + WORDS_W and ly >= y and ly <= y + WORDS_H
	end

	local function press(e: any)
		active:set(true)
		local lx, ly = e.local_x, e.local_y
		-- (A press in the words being written is theirs.)
		if in_words(lx, ly) then
			return
		end
		commit_words()
		local p = to_canvas(lx, ly)
		local zoom = view:peek().zoom
		local t = tool:peek()
		if t == "select" then
			local gs, grip = grip_at(lx, ly)
			if gs and grip then
				gesture = { kind = "grip", grip = grip, id = gs.id, before = shapes(), from = p }
				return
			end
			local s = shape_at(shapes(), p, REACH / zoom)
			if s then
				local sel = table.clone(selected:peek())
				if e.shift then
					sel[s.id] = if sel[s.id] then nil else true
					selected:set(sel)
				elseif not sel[s.id] then
					only({ s.id })
				end
				gesture = { kind = "move", before = shapes(), from = p }
			else
				if not e.shift then
					selected:set({})
				end
				gesture = { kind = "marquee", from = p, shift = e.shift }
			end
		elseif t == "pen" then
			gesture = { kind = "pen", points = { p } }
			draft:set({ id = "draft", kind = "stroke", x = 0, y = 0, w = 0, h = 0, points = { p }, text = "", color = color:peek(), z = 0 })
		elseif t == "rect" or t == "ellipse" then
			gesture = { kind = "box", shape = t, from = p }
		elseif t == "arrow" then
			gesture = { kind = "arrow", from = p }
		elseif t == "text" then
			write_at(p, nil)
			tool:set("select")
		end
	end

	local function move(e: any)
		local g = gesture
		if not g then
			return
		end
		local p = to_canvas(e.local_x, e.local_y)
		local zoom = view:peek().zoom
		if g.kind == "pen" then
			-- Smoothed as it's drawn: each point eased toward the pointer, and
			-- none closer than a point and a half on screen.
			local pts = g.points
			local last = pts[#pts]
			local q = { x = last.x + (p.x - last.x) * 0.6, y = last.y + (p.y - last.y) * 0.6 }
			if math.sqrt((q.x - last.x) ^ 2 + (q.y - last.y) ^ 2) >= 1.5 / zoom then
				table.insert(pts, q)
				local d = draft:peek()
				if d then
					local nd = table.clone(d)
					nd.points = table.clone(pts)
					draft:set(nd)
				end
			end
			g.last = p
		elseif g.kind == "box" then
			local x0, y0 = g.from.x, g.from.y
			draft:set({ id = "draft", kind = g.shape, x = math.min(x0, p.x), y = math.min(y0, p.y), w = math.abs(p.x - x0), h = math.abs(p.y - y0), points = {}, text = "", color = color:peek(), z = 0 })
		elseif g.kind == "arrow" then
			draft:set({ id = "draft", kind = "arrow", x = 0, y = 0, w = 0, h = 0, points = { g.from, p }, text = "", color = color:peek(), z = 0 })
		elseif g.kind == "marquee" then
			marquee:set({ x = math.min(g.from.x, p.x), y = math.min(g.from.y, p.y), w = math.abs(p.x - g.from.x), h = math.abs(p.y - g.from.y) })
		elseif g.kind == "move" then
			local dx, dy = p.x - g.from.x, p.y - g.from.y
			local sel = selected:peek()
			local list = {}
			for _, s in g.before do
				table.insert(list, if sel[s.id] then moved(s, dx, dy) else s)
			end
			g.moved = math.abs(dx) + math.abs(dy) > 0
			ch.value:set(list)
		elseif g.kind == "grip" then
			local list = {}
			for _, s in g.before do
				table.insert(list, if s.id == g.id then regripped(s, g.grip, p) else s)
			end
			g.moved = true
			ch.value:set(list)
		end
	end

	local function release(e: any)
		local g = gesture
		if not g then
			return
		end
		gesture = nil
		local p = to_canvas(e.local_x, e.local_y)
		local zoom = view:peek().zoom
		draft:set(nil)
		if g.kind == "pen" then
			local pts = g.points
			if g.last then
				table.insert(pts, g.last)
			end
			local id = new_id()
			add({ id = id, kind = "stroke", x = 0, y = 0, w = 0, h = 0, points = M.simplify(pts, 0.75 / zoom), text = "", color = color:peek(), z = 0 })
		elseif g.kind == "box" then
			local x0, y0 = g.from.x, g.from.y
			local w, h = math.abs(p.x - x0), math.abs(p.y - y0)
			local x, y = math.min(x0, p.x), math.min(y0, p.y)
			if w < MIN_SIDE and h < MIN_SIDE then
				x, y, w, h = x0 - DEFAULT_W / 2, y0 - DEFAULT_H / 2, DEFAULT_W, DEFAULT_H
			end
			local id = new_id()
			add({ id = id, kind = g.shape, x = x, y = y, w = math.max(w, MIN_SIDE), h = math.max(h, MIN_SIDE), points = {}, text = "", color = color:peek(), z = 0 })
			only({ id })
			tool:set("select")
		elseif g.kind == "arrow" then
			if math.sqrt((p.x - g.from.x) ^ 2 + (p.y - g.from.y) ^ 2) * zoom >= MIN_SIDE then
				local id = new_id()
				add({ id = id, kind = "arrow", x = 0, y = 0, w = 0, h = 0, points = { g.from, p }, text = "", color = color:peek(), z = 0 })
				only({ id })
			end
			tool:set("select")
		elseif g.kind == "marquee" then
			local m = marquee:peek()
			marquee:set(nil)
			if m and m.w + m.h > 0 then
				local sel = if g.shift then table.clone(selected:peek()) else {}
				for _, s in shapes() do
					local b = box_of(s)
					if b.x < m.x + m.w and b.x + b.w > m.x and b.y < m.y + m.h and b.y + b.h > m.y then
						sel[s.id] = true
					end
				end
				selected:set(sel)
			end
		elseif (g.kind == "move" or g.kind == "grip") and g.moved then
			-- One step for the whole drag.
			local after = shapes()
			ch.value:set(g.before)
			change(after)
		end
	end

	local function key(e: any): boolean
		if e.editing or writing:peek() then
			return false
		end
		local k = string.lower(e.key or "")
		if e.cmd and k == "z" then
			if e.shift then
				redo()
			else
				undo()
			end
			return true
		end
		if e.ctrl and k == "y" then
			redo()
			return true
		end
		if e.cmd or e.ctrl or e.alt or e.meta then
			return false
		end
		if e.key == "Delete" or e.key == "Backspace" then
			delete()
			return true
		end
		if e.key == "Escape" then
			-- Lets go: what's chosen, the tool, and the scroll (back to the
			-- page).
			selected:set({})
			tool:set("select")
			active:set(false)
			if board_el then
				board_el:blur()
			end
			return true
		end
		if e.shift and (e.key == "!" or e.code == "Digit1") then
			touched:set(false)
			fit()
			return true
		end
		local t = TOOL_OF_KEY[k]
		if t and not e.shift then
			tool:set(t)
			return true
		end
		return false
	end

	local function zoom_about(lx: number, ly: number, factor: number)
		local v = view:peek()
		local z = math.clamp(v.zoom * factor, 0.1, 8)
		local k = z / v.zoom
		touched:set(true)
		view:set({ x = lx - (lx - v.x) * k, y = ly - (ly - v.y) * k, zoom = z })
	end

	local function tool_button(t: any): Node
		return IconButton {
			class = "orch-scratchpad-tool",
			-- (Some of the tools' glyphs aren't in Icon.Name's list: a longer
			-- union is more than the checker takes in the modules naming it.)
			icon = t.icon :: any,
			label = locale.t(`ds.orch.scratchpad.tool.{t.id}`),
			tooltip = `{locale.s(`ds.orch.scratchpad.tool.{t.id}`)}  {string.upper(t.key)}`,
			size = "sm",
			tone = "quiet",
			lit = function(): boolean
				return tool:get() == t.id
			end,
			on_click = function()
				tool:set(t.id)
			end,
		}
	end
	local tools: { Node } = {}
	for _, t in M.TOOLS do
		table.insert(tools, tool_button(t))
	end
	local swatches: { Node } = {}
	for _, token in M.COLORS do
		-- (A dot of its colour, ringed while it's the one drawn with.)
		table.insert(swatches, ui.button {
			class = "orch-scratchpad-swatch",
			label = locale.t(`ds.orch.scratchpad.color.{token}`),
			w = 14,
			h = 14,
			mx = 3,
			rounded = "$radius.full",
			bg = M.ink(token),
			outline = function(): number
				return if color:get() == token then 1 else 0
			end,
			outline_offset = 2,
			outline_color = "$text.secondary",
			on_click = function()
				color:set(token)
				-- (What's chosen takes it too, one step.)
				local sel = selected:peek()
				if next(sel) ~= nil then
					local list = {}
					for _, s in shapes() do
						if sel[s.id] and s.color ~= token then
							s = copy(s)
							s.color = token
						end
						table.insert(list, s)
					end
					change(list)
				end
			end,
		})
	end

	local toolbar = ui.row {
		class = "orch-scratchpad-toolbar",
		w_full = true,
		gap = "$space.1",
		items = "center",
		ui.row { gap = "$zeron.space.hair", items = "center", table.unpack(tools :: { Node }) },
		ui.div { w = 1, h = 16, mx = "$space.2", bg = "$border.subtle" },
		ui.row { gap = "$zeron.space.hair", items = "center", table.unpack(swatches :: { Node }) },
		ui.div { grow = 1 },
		ui.show(function(): boolean
			return next(selected:get()) ~= nil
		end, IconButton {
			id = "scratchpad-delete",
			icon = "trash-bin-minimalistic",
			label = locale.t("ds.orch.scratchpad.delete"),
			size = "sm",
			tone = "quiet",
			on_click = delete,
		}),
		IconButton {
			id = "scratchpad-undo",
			icon = "undo" :: any,
			label = locale.t("ds.orch.scratchpad.undo_step"),
			size = "sm",
			tone = "quiet",
			disabled = function(): boolean
				return steps:get().undo == 0
			end,
			on_click = undo,
		},
		IconButton {
			id = "scratchpad-redo",
			icon = "redo" :: any,
			label = locale.t("ds.orch.scratchpad.redo"),
			size = "sm",
			tone = "quiet",
			disabled = function(): boolean
				return steps:get().redo == 0
			end,
			on_click = redo,
		},
	}

	local function marks(): Node
		local out: { Node } = {}
		for i = 0, 10 do
			for j = 0, 10 do
				table.insert(out, ui.div {
					id = `scratchpad-at-{i}-{j}`,
					absolute = true,
					w = 12,
					h = 12,
					left = function(): number
						return size:get().w * i / 10 - 6
					end,
					top = function(): number
						return size:get().h * j / 10 - 6
					end,
				})
			end
		end
		return ui.column { absolute = true, left = 0, top = 0, right = 0, bottom = 0, table.unpack(out :: { Node }) }
	end

	local board = ui.column {
		id = props.id or "scratchpad-board",
		class = "orch-scratchpad-board",
		relative = true,
		w_full = true,
		h = props.h,
		grow = props.grow,
		min_h = 0,
		overflow = "hidden",
		rounded = "$radius.lg",
		bg = fill,
		focusable = true,
		focus = { outline = 0 },
		-- (Its pointer: the tool's, restyled in Rust as the tool changes.)
		variants = CURSOR { tool = function(): string
			return tool:get()
		end },
		on_resize = function(s: { w: number, h: number })
			local now = size:peek()
			if math.abs(s.w - now.w) >= 0.5 or math.abs(s.h - now.h) >= 0.5 then
				size:set({ w = s.w, h = s.h })
			end
		end,
		on_press = press,
		on_pointer_move = move,
		on_release = release,
		on_double_click = function(e: any)
			if tool:peek() ~= "select" then
				return
			end
			local p = to_canvas(e.local_x, e.local_y)
			local s = shape_at(shapes(), p, REACH / view:peek().zoom)
			if s and s.kind ~= "stroke" then
				write_at(p, s)
			elseif not s then
				write_at(p, nil)
			end
		end,
		on_mount = function(el: any)
			board_el = el
		end,
		on_blur = function()
			active:set(false)
		end,
		-- (A press elsewhere takes the pointer out first: it lets go then,
		-- focus staying for the tool keys. zgx has no press-outside event,
		-- and a press on a plain part of the page moves no focus.)
		on_hover = function(inside: boolean)
			if not inside then
				active:set(false)
			end
		end,
		on_wheel = function(e: any): boolean?
			if not active:peek() and not (e.cmd or e.ctrl) then
				return nil
			end
			if e.cmd or e.ctrl then
				zoom_about(e.local_x, e.local_y, math.exp(-e.dy * 0.01))
			else
				local v = view:peek()
				touched:set(true)
				view:set({ x = v.x - e.dx, y = v.y - e.dy, zoom = v.zoom })
			end
			return true
		end,
		on_pinch = function(e: any): boolean?
			zoom_about(e.local_x, e.local_y, 1 + e.scale)
			return true
		end,
		-- The drawing, in its own units, the view moving it.
		ui.canvas {
			class = "orch-scratchpad-canvas",
			absolute = true,
			left = 0,
			top = 0,
			right = 0,
			bottom = 0,
			pointer_events = "none",
			view = view,
			bg_grid = { step = 24, min_step = 12, dot = 1, color = "$border.subtle" },
			label = function(): string
				return locale.s("ds.orch.scratchpad.drawing_label", { n = #ch.value:get() })
			end,
			draw = function(c: any)
				local editing = writing:get()
				for _, s in by_z(ch.value:get()) do
					M.draw_shape(c, s, editing ~= nil and editing.id == s.id)
				end
			end,
		},
		-- What's chosen, its grips and what a gesture draws: in screen
		-- points, so they keep their size as the view zooms (a few marks;
		-- the drawing itself is the canvas above, moved by its view).
		-- zgx-lint: allow(canvas-view)
		ui.canvas {
			class = "orch-scratchpad-overlay",
			absolute = true,
			left = 0,
			top = 0,
			right = 0,
			bottom = 0,
			pointer_events = "none",
			draw = function(c: any)
				local v = view:get()
				local function sx(x: number): number
					return x * v.zoom + v.x
				end
				local function sy(y: number): number
					return y * v.zoom + v.y
				end
				local d = draft:get()
				if d then
					local pts = {}
					for i, q in d.points do
						pts[i] = { x = sx(q.x), y = sy(q.y) }
					end
					local scaled = table.clone(d)
					scaled.points = pts
					scaled.x, scaled.y, scaled.w, scaled.h = sx(d.x), sy(d.y), d.w * v.zoom, d.h * v.zoom
					scaled.stroke = (d.stroke or LINE) * v.zoom
					M.draw_shape(c, scaled)
				end
				local m = marquee:get()
				if m then
					c:rect(sx(m.x), sy(m.y), m.w * v.zoom, m.h * v.zoom, "$selection", 2)
				end
				local sel = selected:get()
				local list = {}
				for _, s in ch.value:get() do
					if sel[s.id] then
						table.insert(list, s)
					end
				end
				for _, s in list do
					local b = box_of(s)
					local x, y = sx(b.x) - 4, sy(b.y) - 4
					local w, h = b.w * v.zoom + 8, b.h * v.zoom + 8
					c:stroke(ui.path():rect(x, y, w, h, 4), "$accent", 1)
				end
				if #list == 1 then
					for _, g in grips_of(list[1]) do
						local gx, gy = sx(g.x), sy(g.y)
						c:rect(gx - 4, gy - 4, 8, 8, fill(), 2)
						c:stroke(ui.path():rect(gx - 4, gy - 4, 8, 8, 2), "$accent", 1)
					end
				end
			end,
		},
		ui.show(function(): boolean
			return #ch.value:get() == 0 and draft:get() == nil and writing:get() == nil
		end, ui.column {
			absolute = true,
			left = 0,
			top = 0,
			right = 0,
			bottom = 0,
			items = "center",
			justify = "center",
			pointer_events = "none",
			ui.text { locale.t("ds.orch.scratchpad.no_drawing"), text_size = "$font.size.sm", color = "$text.tertiary" },
		}),
		-- (Under the words being written: they take their presses.)
		ui.show(function(): boolean
			return probe:get()
		end, marks()),
		-- A text being written, where it goes.
		ui.show(function(): boolean
			return writing:get() ~= nil
		end, ui.input {
			id = "scratchpad-words",
			class = "orch-scratchpad-words",
			absolute = true,
			left = function(): number
				local w, v = writing:get(), view:get()
				return if w then w.x * v.zoom + v.x - 6 else 0
			end,
			top = function(): number
				local w, v = writing:get(), view:get()
				return if w then w.y * v.zoom + v.y - 6 else 0
			end,
			w = WORDS_W,
			h = WORDS_H,
			value = words,
			label = locale.s("ds.orch.scratchpad.words"),
			text_size = "$font.size.md",
			bg = "$zeron.settings.card",
			rounded = "$radius.sm",
			-- (Once the press that placed it has given the board focus.)
			on_mount = function(el: any)
				ui.after(0, function()
					el:focus()
				end)
			end,
			on_key = function(e: any): boolean
				if e.key == "Enter" then
					commit_words()
					return true
				elseif e.key == "Escape" then
					writing:set(nil)
					return true
				end
				return false
			end,
			on_blur = commit_words,
		}),
	}

	return ui.column {
		class = "orch-scratchpad-draw",
		w_full = true,
		grow = props.grow,
		min_h = 0,
		gap = "$space.2",
		on_key = key,
		toolbar,
		board,
	}
end)

return M

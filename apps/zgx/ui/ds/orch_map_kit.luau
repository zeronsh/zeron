-- What the Map's three views (ds/orch_map) draw with: a token's ink at an
-- alpha for a canvas, and the platform glyph a machine wears.
--
--   kit.ink(token, alpha?)   `#rrggbbaa` for a canvas, from a theme token
--                            ("zeron.status.failed"), followed (a theme
--                            change paints again)
--   kit.platform_icon(d)     a device's glyph
--   kit.platform_word(d)     its platform, as the Map names it
--   kit.since(t)             seconds since `t`, moving on with the clock
--   kit.held(chat)           a thread's state and activity as shown, each
--                            held 300 ms at least (ds/orch_hold; in a build)
--   kit.MenuField { menu, register, key, hit, center, child }
--                            `child` with a secondary click's menu: what
--                            `hit(x, y)` finds there (an id) and `menu(id)`
--                            offers; `register(key, open)` hands the Map a
--                            way to open it for an id (`center(id)`: where)
--   kit.ref_drag(world, id, title?)
--                            a row's drag source props: a reference to the
--                            thread or orchestrator `id()` is (ds/orch_ref),
--                            for Capture's composer to take
--   kit.Tiled { class, h, draw }   a tall drawing as canvases a band high
--                            (each under zgx's 32 MiB raster limit)
--   kit.path()               `ui.path()` that moves with a band

local status = require("@self/status")
local ContextMenu = require("@self/context_menu")
local locale = require("@self/locale")
local hold = require("@self/orch_hold")
local model = require("@self/orch_map_model")
local Ref = require("@self/orch_ref")

local M = {}

local MADE: { [string]: string } = {}

local function parse(v: any): (string, number)
	if type(v) ~= "string" or string.sub(v, 1, 1) ~= "#" or #v < 7 then
		return "#808080", 1
	end
	local a = if #v >= 9 then (tonumber(string.sub(v, 8, 9), 16) or 255) / 255 else 1
	return string.sub(v, 1, 7), a
end

function M.ink(token: string, alpha: number?): string
	local theme = ui.theme.current:get()
	local key = `{theme}/{token}/{alpha or 1}`
	local kept = MADE[key]
	if kept then
		return kept
	end
	local base, a = parse(ui.theme.get(token))
	local out = base .. string.format("%02x", math.clamp(math.floor(a * (alpha or 1) * 255 + 0.5), 0, 255))
	MADE[key] = out
	return out
end

function M.platform_icon(d: any): any
	if d and (d.cloud == true or d.platform == "cloud") then
		return "cloud"
	elseif d and d.platform == "macos" then
		return "laptop"
	end
	return "monitor"
end

function M.platform_word(d: any): string
	if d and d.platform == "macos" then
		return "macOS"
	elseif d and d.platform == "windows" then
		return "Windows"
	elseif d and d.platform == "linux" then
		return "Linux"
	end
	return (d and d.platform) or ""
end

M.since = status.since

-- A thread's state word and its activity (while it works) as shown: what
-- it is, and what it does, each held 300 ms at least; the activity's label
-- and time read live while its kind stands.
function M.held(chat: () -> any): (() -> string, () -> any)
	local shown = hold.value(function(): any
		local c = chat()
		local s = model.status_of(c)
		local a = c and c.activity
		return { status = s, kind = if s == "working" and a then a.kind else nil }
	end)
	local function st(): string
		return shown().status
	end
	local function activity(): any
		local k = shown().kind
		if not k then
			return nil
		end
		local a = (chat() or {}).activity
		return if a and a.kind == k then a else { kind = k }
	end
	return st, activity
end

export type MenuProps = {
	menu: ((id: string) -> { any }?)?,
	register: ((key: string, open: ((id: string) -> boolean)?) -> ())?,
	key: string,
	hit: (x: number, y: number) -> string?,
	center: (id: string) -> (number?, number?),
	child: Node,
}

function M.MenuField(p: MenuProps): Node
	local menu = p.menu
	if not menu then
		return p.child
	end
	local opener: ((x: number, y: number) -> boolean)? = nil
	local origin: any = nil
	if p.register then
		p.register(p.key, function(id: string): boolean
			local x, y = p.center(id)
			if not (opener and origin and x and y) then
				return false
			end
			return (opener :: any)(origin.x + x, origin.y + (y :: number)) ~= false
		end)
		ui.on_cleanup(function()
			(p.register :: any)(p.key, nil)
		end)
	end
	return ContextMenu {
		items = function(at: any): { any }?
			local id = p.hit(at.local_x, at.local_y)
			return if id then menu(id) else nil
		end,
		hook = function(open: any)
			opener = open
		end,
		label = locale.t("ds.orch.map.menu"),
		w_full = true,
		-- (Where it is in the window: a menu opened for an id hangs there.)
		ui.column {
			w_full = true,
			on_visible = function(_v: boolean, el: Element)
				origin = el
			end,
			p.child,
		},
	}
end

-- What `id` is on the Map, as a reference: an orchestrator (the Brain dump
-- too), else a thread.
function M.ref_of(world: () -> any, id: string, title: string?): any
	local w = world() or {}
	local now = w.now or {}
	for _, o in now.orchestrators or {} do
		if o.id == id then
			return Ref.orchestrator(o, id)
		end
	end
	return Ref.thread(now, (w.chats or {})[id], id, title)
end

function M.ref_drag(world: () -> any, id: () -> string, title: (() -> string?)?): { [string]: any }
	return Ref.source(function(): any
		return M.ref_of(world, id(), if title then title() else nil)
	end)
end

-- A tall drawing as canvases one band high each, stacked: zgx rasterizes a
-- canvas whole, at the display's scale, up to 32 MiB a canvas, so one canvas
-- the height of a long Timeline or Topology (hundreds of lanes) would be
-- refused, and with it the window's frame. A band stays well under that at
-- any width and scale the Map has.
--
--   kit.Tiled { class, h = function() field height end,
--               draw = function(c, w, h, top, bottom) end }
--
-- `draw` draws in the field's own coordinates; `c` is the band's canvas
-- moved up by `top` (text only in the band it starts in, as a canvas's
-- text isn't clipped). `top`/`bottom` are the band's, to skip what's out.
-- Paths go through `kit.path()` (as `ui.path()`) so they move with it.
M.TILE_H = 512

local Rec = {}
Rec.__index = Rec
function M.path(): any
	return setmetatable({ ops = {} }, Rec)
end
for _, op in { "move", "line", "quad", "cubic", "arc", "rect", "ellipse", "close" } do
	Rec[op] = function(self: any, ...: any): any
		table.insert(self.ops, { op, table.pack(...) })
		return self
	end
end
-- Which arguments of each are a y.
local YS: { [string]: { number } } = { move = { 2 }, line = { 2 }, quad = { 2, 4 }, cubic = { 2, 4, 6 }, arc = { 2 }, rect = { 2 }, ellipse = { 2 }, close = {} }
local function built(p: any, dy: number): any
	if getmetatable(p) ~= Rec then
		return p
	end
	local out = ui.path()
	for _, o in p.ops do
		local name, a = o[1], table.clone(o[2])
		for _, i in YS[name] do
			a[i] -= dy
		end
		out = (out :: any)[name](out, table.unpack(a, 1, a.n))
	end
	return out
end
M.built = built

local function shifted(c: any, top: number, bottom: number): any
	local s = {}
	function s:text(t: string, x: number, y: number, o: any?)
		if y >= top and y < bottom then
			c:text(t, x, y - top, o)
		end
	end
	function s:measure_text(t: string, o: any?): (number, number)
		return c:measure_text(t, o)
	end
	function s:rect(x: number, y: number, w: number, h: number, paint: any, r: number?)
		if y < bottom and y + h > top then
			c:rect(x, y - top, w, h, paint, r)
		end
	end
	function s:circle(x: number, y: number, r: number, paint: any)
		if y - r < bottom and y + r > top then
			c:circle(x, y - top, r, paint)
		end
	end
	function s:line(x1: number, y1: number, x2: number, y2: number, paint: any, w: number?, o: any?)
		if math.min(y1, y2) <= bottom and math.max(y1, y2) >= top then
			c:line(x1, y1 - top, x2, y2 - top, paint, w, o)
		end
	end
	function s:fill(p: any, paint: any, rule: string?)
		c:fill(built(p, top), paint, rule)
	end
	function s:stroke(p: any, paint: any, w: number?, o: any?)
		c:stroke(built(p, top), paint, w, o)
	end
	function s:clip(p: any)
		c:clip(built(p, top))
	end
	function s:restore()
		c:restore()
	end
	return s
end
M.shifted = shifted

function M.Tiled(p: { class: string?, h: () -> number, draw: (c: any, w: number, h: number, top: number, bottom: number) -> () }): Node
	local tiles = ui.computed(function(): { number }
		local out = {}
		for i = 0, math.max(0, math.ceil(p.h() / M.TILE_H) - 1) do
			table.insert(out, i)
		end
		return out
	end)
	return ui.each(tiles, function(i: number): Node
		local top = i * M.TILE_H
		return ui.canvas {
			class = p.class,
			absolute = true,
			left = 0,
			right = 0,
			top = top,
			h = function(): number
				return math.max(1, math.min(M.TILE_H, p.h() - top))
			end,
			pointer_events = "none",
			draw = function(c: any, w: number, _h: number)
				p.draw(shifted(c, top, top + M.TILE_H), w, p.h(), top, top + M.TILE_H)
			end,
		}
	end, { key = function(i: number): string
		return tostring(i)
	end, absolute = true, left = 0, top = 0, right = 0, bottom = 0, pointer_events = "none" })
end

return M

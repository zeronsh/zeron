-- An agent's visual (`show_visual`) in a reply, drawn with this design
-- system. The agent's Luau script runs in the shipping app's own sandbox on
-- the host (src/visuals.rs, crates/visual): no I/O, its memory capped, each
-- call on a time budget, its randomness seeded from the visual's id. Its
-- view comes here as plain data, a tree of nodes, and each is drawn as one
-- of ours:
--
--   column, row, grid   our layout, the script's gaps and alignment; a row
--                       the reader can click or scan washes under the
--                       pointer, the wash hanging past its text
--   text                the transcript's type scale (`zeron.visual.text.*`)
--   stat                ds/stat: a headline figure over its label
--   table               ds/data_table: frameless, hairlines, a row wash
--   chart               ds/chart: bars, lines, areas, scatter
--   button, toggle,     ds Button, Switch, Slider; tabs a Segmented control
--   slider, tabs        (a Select for many or long options)
--   link                the accent, opened where the app opens links
--   rule, space         a hairline, room
--   canvas              zgx's canvas: the script's drawing, in our tokens
--   scene, shader       rendered on the GPU off this thread (the shipping
--                       app's crates/visual-gpu), shown as an image
--
-- Colours are the script's words for our tokens (ds/visual_ink), so a
-- visual follows the theme and any template. Its input goes back to the
-- script's handlers (`visuals.event`), and it is drawn again only when its
-- own tree changes shape: values, labels and colours follow in place, so a
-- slider being dragged keeps its drag while the script renders under it.
-- A script that fails, or runs past its budget, shows one quiet line, as a
-- picture that can't be shown does (its message only behind a secondary
-- click, Copy error); the transcript around it carries on.
--
--   Visual { id = "v:…", path = "/…/v_…-visual.luau", title = "Latency", device = nil }
--   (`source` in place of `path`: a fixture's script, run as is.)

local Button = require("@self/button")
local Switch = require("@self/switch")
local Slider = require("@self/slider")
local Segmented = require("@self/segmented")
local Select = require("@self/select")
local Icon = require("@self/icon")
local Stat = require("@self/stat")
local DataTable = require("@self/data_table")
local Chart = require("@self/chart")
local ink = require("@self/visual_ink")
local motion = require("@self/motion")
local status = require("@self/status")
local locale = require("@self/locale")
local Links = require("@ds/links")
local ReplyPicture = require("@self/reply_picture")

export type Props = {
	id: string,
	path: string?,
	title: string?,
	device: string?,
	source: string?,
}

-- What the host says of every visual: `{ [id] = { rev, gen } }`.
local REVS = ui.shared("visuals", {} :: { [string]: { rev: number, gen: number } })

-- A canvas or scene that animates draws at most this often (ms).
local FRAME = 33

-- What can be built at a size of its own in a row; the rest fill its spare
-- width there (the shipping app's "fluid" nodes).
local FLUID = { slider = true, chart = true, canvas = true, scene = true, shader = true, table = true, rule = true }

local function raw(n: number?, why: string): any
	return if n == nil then nil else ui.raw(n, why)
end

-- The tree's shape: everything a build fixes (kinds, structure, layout),
-- none of what follows in place (text, values, labels, colours, handlers).
local FOLLOWS = {
	t = true,
	value = true,
	label = true,
	delta = true,
	tone = true,
	color = true,
	click = true,
	change = true,
	draw = true,
	series = true,
	x = true,
	yaxis = true,
	xaxis = true,
	fmt = true,
	xfmt = true,
	url = true,
	yaw = true,
	pitch = true,
	zoom = true,
	spin = true,
}

local function signature(v: any, out: { string })
	if type(v) ~= "table" then
		table.insert(out, tostring(v))
		return
	end
	if #v > 0 then
		table.insert(out, "[")
		for _, item in v do
			signature(item, out)
		end
		table.insert(out, "]")
		return
	end
	local keys = {}
	for k in v do
		if not FOLLOWS[k] then
			table.insert(keys, k)
		end
	end
	table.sort(keys)
	table.insert(out, "{")
	for _, k in keys do
		table.insert(out, k)
		table.insert(out, "=")
		signature(v[k], out)
	end
	-- (Handlers come and go with the shape: whether there is one counts.)
	if v.click ~= nil then
		table.insert(out, "click")
	end
	if v.spans then
		for _, s in v.spans do
			table.insert(out, if s.color then "c" else "-")
		end
	end
	table.insert(out, "}")
end

-- Every node by its path.
local function index(node: any, into: { [string]: any })
	into[node.p] = node
	for _, child in node.children or {} do
		index(child, into)
	end
	for _, row in node.rows or {} do
		for _, cell in row do
			index(cell, into)
		end
	end
end

type Ctx = {
	id: string,
	title: () -> string,
	-- A node by path, tracked (`at`) or not (`peek`).
	at: (path: string) -> () -> any,
	peek: (path: string) -> any,
	-- Every render's number: canvases draw again with it.
	gen: () -> number,
	event: (handler: number?, arg: any?) -> (),
	-- A failure while drawing (a canvas's, a scene's), shown under it.
	drew: (error: string?) -> (),
}

local build: (n: any, ctx: Ctx, parent: string) -> Node

-- ---- text -------------------------------------------------------------------

local ALIGN_TEXT = { start = "start", center = "center", ["end"] = "right" }

local function text(n: any, ctx: Ctx): Node
	local get = ctx.at(n.p)
	local size, line = ink.size(n.size)
	local props: { [any]: any } = {
		literal = true,
		wrap = not n.truncate,
		truncate = n.truncate,
		lines = if n.truncate then 1 else nil,
		min_w = 0,
		text_size = size,
		line_height = line,
		weight = ink.WEIGHTS[n.weight] or "$font.weight.regular",
		font = if n.mono then "$font.mono" else nil,
		italic = n.italic,
		align = ALIGN_TEXT[n.align] or "start",
		color = function(): any
			local now = get()
			return ink.paint(now and now.color)
		end,
	}
	local spans = n.spans or {}
	local plain = #spans == 1 and not spans[1].weight and not spans[1].color and not spans[1].mono and not spans[1].italic
	if plain or #spans == 0 then
		props[1] = function(): string
			local now = get()
			local s = now and now.spans and now.spans[1]
			return if s then s.t else ""
		end
		return ui.text(props)
	end
	for i, s in spans do
		props[i] = ui.span {
			function(): string
				local now = get()
				local live = now and now.spans and now.spans[i]
				return if live then live.t else ""
			end,
			weight = if s.weight then ink.WEIGHTS[s.weight] else nil,
			font = if s.mono then "$font.mono" else nil,
			italic = s.italic,
			color = if s.color
				then function(): any
					local now = get()
					local live = now and now.spans and now.spans[i]
					return ink.paint(live and live.color)
				end
				else nil,
		}
	end
	return ui.rich(props)
end

-- ---- controls -------------------------------------------------------------------

-- A control's own value, kept in step with the script's: the tree's value
-- becomes the control's, and the reader's change goes to the handler.
local function synced(get: () -> any, peek: () -> any, ctx: Ctx, initial: any): State<any>
	local value = ui.use_state(initial)
	ui.effect(function()
		local now = get()
		if now and now.value ~= nil and now.value ~= value:peek() then
			value:set(now.value)
		end
	end)
	ui.effect(function()
		local v = value:get()
		local now = peek()
		if now and v ~= now.value then
			ctx.event(now.change, v)
		end
	end)
	return value
end

local Tabs = ui.component(function(props: { n: any, ctx: Ctx }): Node
	local n, ctx = props.n, props.ctx
	local get, peek = ctx.at(n.p), function()
		return ctx.peek(n.p)
	end
	local value = synced(get, peek, ctx, n.value)
	local options = {}
	local long = #n.options > 4
	for _, o in n.options do
		table.insert(options, { value = o, label = o })
		if #o > 14 then
			long = true
		end
	end
	if long then
		return Select { value = value, options = options, size = "sm", name = ctx.title() }
	end
	return Segmented { value = value, options = options, label = ctx.title() }
end)

local Toggle = ui.component(function(props: { n: any, ctx: Ctx }): Node
	local n, ctx = props.n, props.ctx
	local get, peek = ctx.at(n.p), function()
		return ctx.peek(n.p)
	end
	local value = synced(get, peek, ctx, n.value == true)
	local function label(): string
		local now = get()
		return if now then now.label or "" else ""
	end
	return ui.row {
		class = "visual-toggle",
		items = "center",
		gap = "$space.3",
		Switch { value = value, label = label() },
		ui.text {
			label,
			literal = true,
			cursor = "pointer",
			-- (The switch is named by it: one control, not two.)
			aria_hidden = true,
			text_size = "$zeron.visual.text.body.size",
			line_height = "$zeron.visual.text.body.line",
			color = "$text.primary",
			on_click = function()
				value:toggle()
			end,
		},
	}
end)

local VisualSlider = ui.component(function(props: { n: any, ctx: Ctx }): Node
	local n, ctx = props.n, props.ctx
	local get, peek = ctx.at(n.p), function()
		return ctx.peek(n.p)
	end
	local value = synced(get, peek, ctx, n.value)
	local function label(): string
		local now = get()
		return if now then now.label or "" else ""
	end
	return ui.column {
		class = "visual-slider",
		w_full = true,
		min_w = 0,
		gap = "$space.1",
		ui.show(
			function(): boolean
				return label() ~= ""
			end,
			ui.text {
				label,
				literal = true,
				truncate = true,
				text_size = "$zeron.visual.text.sm.size",
				line_height = "$zeron.visual.text.sm.line",
				color = "$text.secondary",
			}
		),
		Slider {
			value = value,
			min = n.min,
			max = n.max,
			step = n.step,
			label = n.label or ctx.title(),
			w_full = true,
			format = function(v: number): string
				local now = peek()
				return ink.format(v, now and now.fmt)
			end,
		},
	}
end)

-- ---- when it can't be shown ----------------------------------------------------

-- A visual that can't be shown, or stopped: one quiet line, as a picture
-- that can't be shown has (ds/reply_picture `Line`), nothing raw in the
-- reply. A reason a reader can use (plain words, not a script's fault) is
-- its tooltip; the script's own message is a secondary click away (Copy
-- error), for whoever is fixing it. A new message makes a new line.
-- (`stopped`: it shows, its last good view above; it stopped answering.)
local function problem(message: () -> string, stopped: boolean?): Node
	return ui.presence(message, function(raw: string): Node
		return ui.row {
			class = "visual-error",
			w_full = true,
			context_menu = if raw ~= ""
				then function(): { Node }?
					return {
						ui.menu_item {
							label = locale.t("ds.visual.copy_error"),
							on_click = function()
								ui.clipboard.write(raw)
							end,
						},
					}
				end
				else nil,
			ReplyPicture.Line {
				icon = "widget",
				words = locale.s(if stopped then "ds.visual.stopped" else "ds.transcript.visual_failed"),
				reason = ReplyPicture.plain(raw),
			},
		}
	end, { w_full = true })
end

-- ---- canvases -------------------------------------------------------------------

local function text_size(name: string?): number
	local size = ink.size(name)
	local v = ui.theme.get(string.sub(size, 2))
	return if type(v) == "number" then v else 15
end

local WEIGHT = { regular = 400, medium = 500, semibold = 600, bold = 700 }
local BASELINE = { start = "top", center = "middle", ["end"] = "bottom" }

-- The script's drawing on our canvas: its fills, strokes and text in our
-- tokens. (Polygons and polylines, as crates/visual records them.)
local function paint(c: any, cmds: { any })
	for _, cmd in cmds do
		local k = cmd.k
		-- (Circles, rectangles and lines as zgx draws them on the GPU: no
		-- path built, nothing rasterized.)
		if k == "c" then
			c:circle(cmd.x, cmd.y, cmd.r, ink.canvas(cmd.c))
		elseif k == "r" then
			c:rect(cmd.x, cmd.y, cmd.w, cmd.h, ink.canvas(cmd.c))
		elseif k == "l" then
			local pts = cmd.pts
			c:line(pts[1], pts[2], pts[3], pts[4], ink.canvas(cmd.c), cmd.w or 1, {
				cap = "round",
				dash = if cmd.dash then { cmd.dash, cmd.dash } else nil,
			})
		elseif k == "m" then
			-- A curve in runs whose x doesn't turn back (src/visuals.rs): each
			-- one zgx's canvas strokes on the GPU. Where two runs meet the
			-- curve runs straight up or down, so their square-cut ends close
			-- on each other with no seam and no doubled ink.
			local paint = ink.canvas(cmd.c)
			local runs = cmd.runs or {}
			local cap = if #runs == 1 then "round" else "butt"
			for _, pts in runs do
				if #pts >= 4 then
					local p = ui.path():move(pts[1], pts[2])
					for i = 3, #pts - 1, 2 do
						p = p:line(pts[i], pts[i + 1])
					end
					c:stroke(p, paint, cmd.w or 1, { join = "round", cap = cap })
				end
			end
		elseif k == "t" then
			c:text(cmd.t, cmd.x, cmd.y, {
				size = text_size(cmd.size),
				weight = WEIGHT[cmd.weight] or 400,
				color = ink.canvas(cmd.c),
				font = if cmd.mono then "mono" else nil,
				align = cmd.align,
				baseline = BASELINE[cmd.base] or "middle",
			})
		else
			local pts = cmd.pts
			if #pts >= 4 then
				local p = ui.path():move(pts[1], pts[2])
				for i = 3, #pts - 1, 2 do
					p = p:line(pts[i], pts[i + 1])
				end
				if k == "f" or cmd.closed then
					p = p:close()
				end
				if k == "f" then
					c:fill(p, ink.canvas(cmd.c))
				else
					c:stroke(p, ink.canvas(cmd.c), cmd.w or 1, {
						join = "round",
						cap = "round",
						dash = if cmd.dash then { cmd.dash, cmd.dash } else nil,
					})
				end
			end
		end
	end
end

-- Whether the window shows (not hidden, minimized or covered); true where
-- a host doesn't say.
local function window_shown(): boolean
	local ok, shown = pcall(function()
		return ui.window.shown:get()
	end)
	return not ok or shown ~= false
end

-- Seconds since `start`, stepped while something animates, only while it
-- shows (`visible`: on screen, in a window that shows), and holding still
-- with motion reduced. Nothing ticks for one scrolled away.
local function clock(animates: () -> boolean, visible: State<boolean>): State<number>
	local started = ui.now()
	local t = ui.use_state(0)
	ui.effect(function()
		if animates() and visible:get() and window_shown() and not motion.reduced() then
			ui.every(FRAME, function()
				t:set((ui.now() - started) / 1000)
			end)
		end
	end)
	return t
end

local Canvas = ui.component(function(props: { n: any, ctx: Ctx }): Node
	local n, ctx = props.n, props.ctx
	local get = ctx.at(n.p)
	local pointer = ui.use_state(nil :: any)
	local held = false
	-- Whether the drawing looked at the pointer (it draws again as it moves
	-- only then).
	local reads_pointer = false
	-- (Told at its first layout, and as it comes and goes: nothing moves
	-- before it's seen.)
	local visible = ui.use_state(false)
	local t = clock(function(): boolean
		local now = get()
		return now ~= nil and now.animate == true
	end, visible)
	local function at(e: any): any
		return { x = e.local_x, y = e.local_y, down = held }
	end
	return ui.canvas {
		class = "visual-canvas",
		w_full = true,
		h = n.h,
		label = ctx.title(),
		cursor = if n.click then "pointer" else nil,
		on_visible = function(shows: boolean)
			visible:set(shows)
		end,
		draw = function(c: any, w: number, h: number)
			ctx.gen()
			local now = get()
			if not now then
				return
			end
			local p = if reads_pointer then pointer:get() else pointer:peek()
			local arg = { id = ctx.id, h = now.draw, w = w, ht = h, t = if now.animate then t:get() else 0 }
			if p then
				arg.px, arg.py, arg.down = p.x, p.y, p.down
			end
			local ok, drawn = pcall(ui.call_sync, "visuals.draw", arg)
			if not ok or type(drawn) ~= "table" then
				return
			end
			if drawn.error then
				ctx.drew(drawn.error)
				return
			end
			ctx.drew(nil)
			reads_pointer = drawn.pointer == true
			paint(c, drawn.cmds or {})
		end,
		on_pointer_move = function(e: any)
			pointer:set(at(e))
		end,
		on_hover = function(inside: boolean)
			if not inside and not held then
				pointer:set(nil)
			end
		end,
		on_press = function(e: any)
			held = true
			pointer:set(at(e))
		end,
		on_release = function(e: any)
			held = false
			pointer:set(at(e))
		end,
		-- (Only one the script hears is a control.)
		on_click = if n.click
			then function(e: any)
				local now = ctx.peek(n.p)
				if now and now.click then
					local p = pointer:peek()
					ctx.event(now.click, { x = e.local_x or (p and p.x) or 0, y = e.local_y or (p and p.y) or 0 })
				end
			end
			else nil,
	}
end)

-- A scene or shader: its frames rendered on the host's GPU thread and shown
-- as an image, one asked for at a time (a newer wish waits for it). The
-- camera turns under a drag; a scene's labels are set as our text over it,
-- a hint under the pointer said in the corner.
local Surface = ui.component(function(props: { n: any, ctx: Ctx }): Node
	local n, ctx = props.n, props.ctx
	local get = ctx.at(n.p)
	local box = ui.ref()
	local src = ui.use_state("")
	local labels = ui.use_state({} :: { any })
	local hints: { any } = {}
	local hint = ui.use_state("")
	local failed = ui.use_state("")
	local yaw = ui.use_state(n.yaw or 0)
	local pitch = ui.use_state(n.pitch or 0)
	local mouse = ui.use_state(nil :: any)
	local drag: any = nil
	local function spinning(): boolean
		local now = get()
		return now ~= nil and (now.spin or 0) ~= 0 and drag == nil
	end
	-- (Told at its first layout, and as it comes and goes: nothing moves
	-- before it's seen.)
	local visible = ui.use_state(false)
	local t = clock(function(): boolean
		local now = get()
		return now ~= nil and (now.animate == true or spinning())
	end, visible)
	local busy, again = false, false
	local function request()
		if busy then
			again = true
			return
		end
		-- (Off screen, nothing is rendered: it asks again as it comes back.)
		if not visible:peek() then
			return
		end
		local now = ctx.peek(n.p)
		local w = box.width or 0
		if not now or w <= 0 then
			return
		end
		busy = true
		local time = t:peek()
		local spin = if motion.reduced_now() then 0 else (now.spin or 0)
		local arg = {
			id = ctx.id,
			p = n.p,
			w = w,
			ht = now.h,
			scale = 2,
			t = time,
			pal = ink.palette(),
			yaw = yaw:peek() + (if drag then 0 else spin * time),
			pitch = pitch:peek(),
			mouse = mouse:peek(),
		}
		local ok, promise = pcall(ui.call, "visuals.frame", arg)
		if not ok or not promise then
			busy = false
			failed:set(if ok then "" else tostring(promise))
			return
		end
		promise:after(function(got: any)
			busy = false
			if type(got) == "table" then
				failed:set("")
				src:set(got.src or "")
				labels:set(got.labels or {})
				hints = got.hints or {}
			end
			if again then
				again = false
				request()
			end
		end, function(err: any)
			busy = false
			again = false
			failed:set(tostring(err))
		end)
	end
	-- A frame for every change it shows: a render, its size, the theme, its
	-- coming into view; then each tick while it moves (which takes the
	-- camera and pointer as they are), else each move of the camera or the
	-- pointer (a shader's) while it's still. At most one frame a tick.
	ui.effect(function()
		ctx.gen()
		local _ = box.width, ui.theme.current:get(), visible:get()
		local now = get()
		if now and (now.animate or spinning()) then
			t:get()
		else
			local _ = yaw:get(), pitch:get(), mouse:get()
		end
		request()
	end)
	ui.on_cleanup(function()
		pcall(ui.invoke, "visuals.release", { id = ctx.id, p = n.p })
	end)
	local function hint_at(x: number, y: number): string
		for _, h in hints do
			local dx, dy = x - h.x, y - h.y
			if dx * dx + dy * dy <= (h.r or 8) ^ 2 then
				return h.t
			end
		end
		return ""
	end
	local size_xs = text_size("xs")
	return ui.column {
		class = if n.k == "scene" then "visual-scene" else "visual-shader",
		w_full = true,
		gap = "$space.2",
		ui.overlay {
			ref = box,
			w_full = true,
			h = n.h,
			cursor = if n.orbit then "grab" else nil,
			label = ctx.title(),
			on_visible = function(shows: boolean)
				visible:set(shows)
			end,
			on_press = function(e: any)
				local spin = if motion.reduced_now() then 0 else ((get() or {}).spin or 0)
				-- (Where the turntable had got to is where the drag starts.)
				yaw:set(yaw:peek() + spin * t:peek())
				drag = { x = e.local_x, y = e.local_y, yaw = yaw:peek(), pitch = pitch:peek() }
				if n.k == "shader" then
					mouse:set({ x = e.local_x, y = e.local_y, down = true, sx = e.local_x, sy = e.local_y })
				end
			end,
			on_pointer_move = function(e: any)
				if drag and n.orbit then
					yaw:set(drag.yaw + (e.local_x - drag.x) * 0.01)
					pitch:set(math.clamp(drag.pitch + (e.local_y - drag.y) * 0.01, -1.5, 1.5))
				end
				if n.k == "shader" then
					local m = mouse:peek()
					mouse:set({ x = e.local_x, y = e.local_y, down = drag ~= nil, sx = m and m.sx, sy = m and m.sy })
				else
					hint:set(hint_at(e.local_x, e.local_y))
				end
			end,
			on_release = function()
				drag = nil
				-- The turntable goes on from where the drag left it.
				if spinning() then
					local spin = if motion.reduced_now() then 0 else ((get() or {}).spin or 0)
					yaw:set(yaw:peek() - spin * t:peek())
				end
				local m = mouse:peek()
				if m then
					mouse:set({ x = m.x, y = m.y, down = false })
				end
			end,
			on_hover = function(inside: boolean)
				if not inside then
					hint:set("")
				end
			end,
			ui.image {
				src = function(): string
					return src:get()
				end,
				w_full = true,
				h_full = true,
				-- (A frame drawn for an earlier size, while the box resizes:
				-- covered, never stretched.)
				fit = "cover",
				label = ctx.title(),
			},
			ui.canvas {
				w_full = true,
				h_full = true,
				pointer_events = "none",
				draw = function(c: any)
					for _, l in labels:get() do
						c:text(l.t, l.x, l.y, {
							size = text_size(l.size),
							color = ink.canvas(l.c),
							align = "center",
							baseline = "bottom",
						})
					end
				end,
			},
			ui.show(
				function(): boolean
					return hint:get() ~= ""
				end,
				ui.text {
					function(): string
						return hint:get()
					end,
					literal = true,
					absolute = true,
					left = 0,
					top = 0,
					pointer_events = "none",
					text_size = size_xs,
					weight = "$font.weight.medium",
					color = "$text.primary",
				}
			),
		},
		ui.show(
			function(): boolean
				return failed:get() ~= ""
			end,
			problem(function(): string
				return failed:get()
			end)
		),
	}
end)

-- ---- layout -------------------------------------------------------------------

local ITEMS = { start = "start", center = "center", ["end"] = "end" }
local JUSTIFY = { start = "start", center = "center", ["end"] = "end", between = "between" }

-- How a child sits in a row: the width it was given, the spare width it
-- takes (asked, or a node with no width of its own), or its own.
local function place(props: { [any]: any }, n: any, parent: string)
	if n.w then
		props.w = raw(n.w, "a visual's width for this, as its script asks")
		props.shrink = 0
	elseif n.grow or (parent == "row" and FLUID[n.k]) then
		props.grow = 1
		props.basis = 0
		props.min_w = 0
	elseif parent == "row" then
		props.shrink = 1
		props.min_w = 0
	end
	return props
end

local function stack(n: any, ctx: Ctx, parent: string): Node
	local row = n.k == "row"
	local kids: { Node } = {}
	for _, child in n.children or {} do
		table.insert(kids, build(child, ctx, n.k))
	end
	local props: { [any]: any } = {
		class = if row then "visual-row" else "visual-column",
		w_full = parent ~= "row",
		min_w = 0,
		gap = raw(n.gap, "a visual's gap, as its script asks"),
		items = if row then (ITEMS[n.align] or "start") elseif n.align == "start" then nil else ITEMS[n.align],
		justify = JUSTIFY[n.justify],
		flex_wrap = row and n.wrap or nil,
		kids,
	}
	if n.hover or n.click then
		local h = ui.theme.get("space.2")
		local v = ui.theme.get("space.1")
		local px = if type(h) == "number" then h else 8
		local py = if type(v) == "number" then v else 4
		-- The wash hangs past the text, which stays where it was.
		props.px, props.py = "$space.2", "$space.1"
		props.mx = ui.raw(-px, "the wash's room, out past the text so the text keeps its edge")
		props.my = ui.raw(-py, "the wash's room, out past the text so the text keeps its edge")
		props.rounded = "$radius.md"
		props.hover = { bg = "$hover" }
		if n.click then
			props.cursor = "pointer"
			props.role = "button"
			props.focusable = true
			props.on_click = function()
				local now = ctx.peek(n.p)
				ctx.event(now and now.click)
			end
			props.on_key = function(e: any): boolean
				if e.key == "Enter" or e.key == " " then
					local now = ctx.peek(n.p)
					ctx.event(now and now.click)
					return true
				end
				return false
			end
		end
	end
	place(props, n, parent)
	return if row then ui.row(props) else ui.column(props)
end

build = function(n: any, ctx: Ctx, parent: string): Node
	local k = n.k
	if k == "column" or k == "row" then
		return stack(n, ctx, parent)
	elseif k == "grid" then
		local kids: { Node } = {}
		for _, child in n.children or {} do
			table.insert(kids, ui.column { min_w = 0, build(child, ctx, "grid") })
		end
		return ui.grid(place({ class = "visual-grid", w_full = true, cols = n.columns, gap = raw(n.gap, "a visual's gap, as its script asks"), kids }, n, parent))
	elseif k == "text" then
		local node = text(n, ctx)
		if parent == "row" and (n.w or n.grow) then
			return ui.column(place({ min_w = 0, node }, n, parent))
		end
		return node
	elseif k == "rule" then
		if parent == "row" then
			return ui.div { w = 1, self = "stretch", shrink = 0, bg = "$border" }
		end
		return ui.div { w_full = true, h = 1, shrink = 0, bg = "$border" }
	elseif k == "space" then
		local s = raw(n.size, "a visual's room, as its script asks")
		return ui.div { w = s, h = s, shrink = 0 }
	elseif k == "stat" then
		local get = ctx.at(n.p)
		local function field(name: string): () -> any
			return function(): any
				local now = get()
				return now and now[name]
			end
		end
		return ui.column(place({
			min_w = 0,
			Stat {
				value = field("value"),
				label = field("label"),
				delta = field("delta"),
				tone = function(): any
					local now = get()
					return if now and now.tone then ink.paint(now.tone) else nil
				end,
			},
		}, n, parent))
	elseif k == "table" then
		local count = 0
		for _, row in n.rows or {} do
			count = math.max(count, #row)
		end
		return ui.column(place({
			w_full = parent ~= "row",
			min_w = 0,
			DataTable {
				columns = n.columns or {},
				count = count,
				rows = #(n.rows or {}),
				hover = n.hover,
				label = ctx.title(),
				cell = function(r: number, c: number): Node
					local cell = (n.rows[r] or {})[c]
					if not cell then
						return ui.div {}
					end
					return build(cell, ctx, "cell")
				end,
			},
		}, n, parent))
	elseif k == "chart" then
		local get = ctx.at(n.p)
		return ui.column(place({
			w_full = parent ~= "row",
			min_w = 0,
			Chart { data = get, height = n.h, label = ctx.title() },
		}, n, parent))
	elseif k == "canvas" then
		return ui.column(place({ w_full = parent ~= "row", min_w = 0, Canvas { n = n, ctx = ctx } }, n, parent))
	elseif k == "scene" or k == "shader" then
		return ui.column(place({ w_full = parent ~= "row", min_w = 0, Surface { n = n, ctx = ctx } }, n, parent))
	elseif k == "button" then
		local get = ctx.at(n.p)
		return Button(place({
			tone = if n.primary then "primary" else "secondary",
			size = "sm",
			on_click = function()
				local now = ctx.peek(n.p)
				ctx.event(now and now.click)
			end,
			ui.text {
				function(): string
					local now = get()
					return if now then now.label else ""
				end,
				literal = true,
			},
		}, n, parent))
	elseif k == "tabs" then
		return ui.row(place({ min_w = 0, Tabs { n = n, ctx = ctx } }, n, parent))
	elseif k == "toggle" then
		return ui.row(place({ min_w = 0, Toggle { n = n, ctx = ctx } }, n, parent))
	elseif k == "slider" then
		return ui.column(place({ w_full = parent ~= "row", min_w = 0, VisualSlider { n = n, ctx = ctx } }, n, parent))
	elseif k == "link" then
		local get = ctx.at(n.p)
		return ui.text(place({
			function(): string
				local now = get()
				return if now then now.label else ""
			end,
			class = "visual-link",
			literal = true,
			role = "link",
			cursor = "pointer",
			text_size = "$zeron.visual.text.body.size",
			line_height = "$zeron.visual.text.body.line",
			color = "$accent",
			hover = { color = "$accent.hover" },
			on_click = function()
				local now = ctx.peek(n.p)
				if now and now.url then
					Links.open(now.url)
				end
			end,
			-- A web link's menu (ds/links), for where it goes now.
			context_menu = function(e: PointerEvent): { Node }?
				local now = ctx.peek(n.p)
				if now and type(now.url) == "string" and Links.web(now.url) then
					return Links.menu_for(now.url)(e)
				end
				return nil
			end,
		}, n, parent))
	end
	return ui.div {}
end

-- ---- the visual ---------------------------------------------------------------

return ui.component(function(props: Props): Node
	local id = props.id
	-- Read and started once (the host keeps it: a reload, the chat opened
	-- again, another window, none starts it twice).
	pcall(function()
		local promise = ui.call("visuals.open", {
			id = id,
			path = props.path,
			title = props.title,
			device = props.device,
			source = props.source,
		})
		if promise then
			promise:after(function() end, function() end)
		end
	end)
	local rev = ui.computed(function(): number
		local r = REVS:get()[id]
		return if r then r.rev else -1
	end)
	local gen = ui.computed(function(): number
		local r = REVS:get()[id]
		return if r then r.gen else -1
	end)
	local snap = ui.computed(function(): any
		rev:get()
		local ok, s = pcall(ui.call_sync, "visuals.get", id)
		return if ok and type(s) == "table" then s else { state = "loading" }
	end)
	local nodes = ui.computed(function(): { [string]: any }
		local into = {}
		local tree = snap:get().tree
		if tree then
			index(tree, into)
		end
		return into
	end)
	local shape = ui.computed(function(): string
		local s = snap:get()
		if s.state ~= "ready" or not s.tree then
			return s.state or "loading"
		end
		local out = { "ready" }
		signature(s.tree, out)
		return table.concat(out, " ")
	end)
	local drew = ui.use_state("")
	local function title(): string
		local t = props.title or snap:peek().title or ""
		return if t ~= "" then t else locale.s("ds.visual.untitled")
	end
	local ctx: Ctx = {
		id = id,
		title = title,
		at = function(path: string): () -> any
			return function(): any
				return nodes:get()[path]
			end
		end,
		peek = function(path: string): any
			return nodes:peek()[path]
		end,
		gen = function(): number
			return gen:get()
		end,
		event = function(handler: number?, arg: any?)
			if type(handler) == "number" then
				pcall(ui.invoke, "visuals.event", { id = id, h = handler, arg = arg })
			end
		end,
		drew = function(error: string?)
			local message = error or ""
			if drew:peek() ~= message then
				-- (Told while drawing: shown once the frame is done.)
				ui.after(0, function()
					drew:set(message)
				end)
			end
		end,
	}
	local function error(): string
		local s = snap:get()
		if s.state == "ready" and s.error then
			return s.error
		end
		return drew:get()
	end
	return ui.column {
		class = "visual",
		role = "group",
		label = title(),
		w_full = true,
		gap = "$space.3",
		ui.presence(function(): string
			return shape:get()
		end, function(key: string): Node
			if key == "loading" then
				return ui.row {
					class = "visual-loading",
					items = "center",
					gap = "$space.2",
					h = "$zeron.visual.text.sm.line",
					status.Spinner { size = 12, ink = "$text.tertiary" },
					ui.text {
						function(): string
							return locale.s("ds.visual.loading", { title = title() })
						end,
						literal = true,
						text_size = "$zeron.visual.text.sm.size",
						line_height = "$zeron.visual.text.sm.line",
						color = "$text.tertiary",
					},
				}
			end
			if key == "failed" then
				return problem(function(): string
					return snap:get().error or ""
				end)
			end
			local tree = snap:peek().tree
			if not tree then
				return ui.div {}
			end
			-- (A tree this can't draw stays in its place, said so: the
			-- transcript round it goes on.)
			local ok, built = pcall(build, tree, ctx, "root")
			if ok then
				return built
			end
			return problem(function(): string
				return tostring(built)
			end)
		end, { w_full = true }),
		ui.show(
			function(): boolean
				return error() ~= "" and shape:get() ~= "failed"
			end,
			problem(error, true)
		),
	}
end)

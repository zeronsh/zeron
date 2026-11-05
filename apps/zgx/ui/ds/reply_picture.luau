-- The pictures a reply shows: those it names in Markdown (`![alt](path)`)
-- and those an agent shows (`show_image`, a generated image). Zeron's
-- reply image row (crates/ui/src/reply_images.rs) is the behaviour; the
-- look is the design system's.
--
-- One alone (`Single`) takes its own shape: as wide as the reading column
-- at most, 420 tall at most, never past its own size. A long page shows its
-- top, cut to half as wide as it is tall and fading out at its foot; a long
-- strip its middle, five times as wide as it is tall. A small one (an icon)
-- sits at its own size in the middle of a 64-point tile, never blown up.
--
-- Several in a row (`Run`, pictures with nothing between them) sit on a
-- tray: a quiet wash with a 16 corner, each picture 4 in from its edge and
-- from the next, so each picture's corner is the tray's less that inset
-- (12, concentric). They lay out in justified rows (up to three a row, as
-- the column allows), each row as tall as fills the tray's width, so they
-- read as one set.
--
-- While a picture comes, its frame holds its place, still and quiet: at the
-- size it will be when that's known (each picture's size is kept once
-- read, from the file's header alone: `ui.image_size`), otherwise the
-- column's width at a screenshot's shape (16:10), which is where most land.
-- When it comes it fades in, and a frame that has to change size eases to
-- it (both at once with Reduce motion). A picture that was already here
-- when its reply was drawn (a reload, a chat opened again, a reply being
-- written) shows at once: nothing replays.
--
-- One that can't be read is a quiet line of its own: what it was and why,
-- a link to its path where the reply's links open (Zeron keeps the link).
-- It is read again on Zeron's backoff (screens/chat/reply_images).
--
-- Under the pointer (or with focus), Copy and, for a file on this Mac, Show
-- in Finder sit at its top right, each a small glass square 6 in from the
-- corner (its corner the frame's less 6). A secondary click has Open, Copy
-- Image, Copy Path and Show in Finder. A click, or Enter, opens it in the
-- lightbox, growing out of it, among the reply's other pictures (← →).
--
-- The reading is the caller's (`pictures`, screens/chat/reply_images): each
-- asks for its file once, and what it is is kept there.
--
--   ReplyPicture.Single { pictures = p, src = "shots/run.png", alt = "Run",
--     group = "reply:42", set = function() return …the reply's… end }
--   ReplyPicture.Run { pictures = p, items = function() return { { src, alt }, … } end,
--     group = "reply:42", set = … }

local Icon = require("@self/icon")
local IconButton = require("@self/icon_button").IconButton
local Lightbox = require("@self/lightbox")
local ContextMenu = require("@self/context_menu")
local control = require("@self/control")
local glass = require("@self/glass")
local Motion = require("@self/motion")
local locale = require("@self/locale")
local toast = require("@self/toast")
-- (By the alias the shell sets its opener on: one instance per path.)
local Links = require("@ds/links")

local M = {}

export type Picture = {
	state: "loading" | "ready" | "failed",
	src: string?,
	error: string?,
	path: string?,
	here: boolean?,
	tries: number?,
	retry: number?,
}
export type Pictures = {
	resolve: (src: string) -> Picture?,
	fetch: (src: string) -> (),
}
-- A picture as the reply wrote it: its `src` (a path) and its name (`alt`,
-- or a generated image's file name).
export type Entry = { src: string, alt: string }
export type Props = {
	pictures: Pictures,
	src: string,
	alt: string,
	-- Its reply's lightbox group, and the reply's pictures in order, read
	-- as it opens.
	group: string,
	set: () -> { Entry },
	-- What opens a link (a picture that can't be shown is one to its path),
	-- and its context menu's entries (ds/markdown's `link_menu`).
	open_link: ((target: string) -> ())?,
	link_menu: ((target: string) -> { Links.Entry }?)?,
}
export type RunProps = {
	pictures: Pictures,
	items: () -> { Entry },
	group: string,
	set: () -> { Entry },
	open_link: ((target: string) -> ())?,
	link_menu: ((target: string) -> { Links.Entry }?)?,
}

local function n(token: string, fallback: number): number
	local v = ui.theme.get(token)
	return if type(v) == "number" then v else fallback
end

-- The measures, the theme's (zeron.prose.picture): the widest and tallest,
-- a placeholder's shape, the corner, a small one's tile, how long or wide
-- before it's cut, the tray's inset and corner, a run's tallest row, and
-- the controls' inset from the corner.
local function measures(): { [string]: number }
	return {
		W = n("zeron.prose.picture.w", 660),
		H = n("zeron.prose.picture.h", 420),
		PLACE = n("zeron.prose.picture.place", 1.6),
		R = n("zeron.prose.picture.radius", 12),
		MIN = n("zeron.prose.picture.min", 64),
		TALL = n("zeron.prose.picture.tall", 0.5),
		WIDE = n("zeron.prose.picture.wide", 5),
		INSET = n("zeron.prose.picture.tray.inset", 4),
		ROW = n("zeron.prose.picture.tray.row", 300),
		CONTROL = n("zeron.prose.picture.control.inset", 6),
	}
end

local function name_of(e: Entry): string
	if e.alt ~= "" then
		return e.alt
	end
	return string.match(e.src, "([^/\\]+)$") or e.src
end

-- Pictures' sizes in pixels, by the file shown, the truest first: as it
-- decoded where it's drawn (`on_load`), else its file's header now
-- (`ui.image_size`, no decode). Never kept by path alone: a path can be
-- written over with another picture (an agent's screenshot taken again),
-- and a size kept from before would stretch the new one. Nil for a file
-- that isn't there or isn't a picture.
local DECODED = ui.state("reply_picture_decoded", {} :: { [string]: { w: number, h: number } })
local function size_of(file: string?): (number?, number?)
	if not file or file == "" then
		return nil, nil
	end
	local d = DECODED:get()[file]
	if d then
		return d.w, d.h
	end
	local ok, w, h = pcall(ui.image_size, file)
	if not ok or type(w) ~= "number" or type(h) ~= "number" or w <= 0 or h <= 0 then
		return nil, nil
	end
	return w, h
end

-- What `file` drew at: put right where its header said otherwise.
local function decoded(file: string, s: { w: number, h: number })
	if file == "" or s.w <= 0 or s.h <= 0 then
		return
	end
	local w, h = size_of(file)
	if w ~= s.w or h ~= s.h then
		DECODED:set_key(file, { w = s.w, h = s.h })
	end
end

-- Files that couldn't be drawn (gone since they were kept, or not a
-- picture after all), with why: as `ui.image`'s `on_error` tells it.
local BROKEN = ui.state("reply_picture_broken", {} :: { [string]: string })

export type Fit = {
	w: number,
	h: number,
	-- How the picture sits in its frame: filling it ("none"), its top
	-- ("top", a long page), its middle ("middle", a long strip), or at its
	-- own size in the middle ("small", the picture `pw` by `ph`).
	crop: "none" | "top" | "middle" | "small",
	pw: number?,
	ph: number?,
}

-- A picture of `nw` by `nh` pixels alone, in at most `maxw` by `maxh`.
function M.fit(nw: number, nh: number, maxw: number, maxh: number): Fit
	local m = measures()
	local a = nw / nh
	local fw, fh = nw, nh
	local crop: "none" | "top" | "middle" = "none"
	if a < m.TALL then
		fw, fh, crop = nw, nw / m.TALL, "top"
	elseif a > m.WIDE then
		fw, fh, crop = nh * m.WIDE, nh, "middle"
	end
	local k = math.min(1, maxw / fw, maxh / fh)
	fw, fh = fw * k, fh * k
	if fw < m.MIN or fh < m.MIN then
		-- Small: whole, at its own size (or the column's), in a tile.
		local k2 = math.min(1, maxw / nw, maxh / nh)
		local pw, ph = nw * k2, nh * k2
		return { w = math.max(pw, m.MIN), h = math.max(ph, m.MIN), crop = "small", pw = pw, ph = ph }
	end
	return { w = fw, h = fh, crop = crop }
end

-- How a run lays out in `inner` points: each picture's place (by its
-- `src`), and the size it takes. `aspects` are the pictures' shapes in
-- order (nil: not known yet).
export type Place = { x: number, y: number, w: number, h: number, crop: "none" | "top" | "small", pw: number?, ph: number? }
function M.layout(list: { { src: string, w: number?, h: number? } }, inner: number): ({ [string]: Place }, number, number)
	local m = measures()
	local gap = m.INSET
	local count = #list
	local per = if inner >= 480 then 3 elseif inner >= 240 then 2 else 1
	local rows = math.max(1, math.ceil(count / per))
	local places: { [string]: Place } = {}
	local y, widest = 0, 0
	local at = 1
	local first_h: number? = nil
	for r = 1, rows do
		local left = count - at + 1
		local k = math.ceil(left / (rows - r + 1))
		local shapes: { number } = {}
		local sum = 0
		for i = at, at + k - 1 do
			local it = list[i]
			local a = if it.w and it.h and it.h > 0 then it.w / it.h else m.PLACE
			a = math.clamp(a, 0.6, 2.2)
			shapes[i] = a
			sum += a
		end
		local raw = (inner - gap * (k - 1)) / sum
		local h = math.min(raw, m.ROW)
		-- (A shorter last row is never taller than the rows above it.)
		if first_h and k < per then
			h = math.min(h, first_h)
		end
		-- (Whether the row spans the tray: its last picture then ends on
		-- the edge.)
		local spans = h >= raw
		h = math.floor(h)
		first_h = first_h or h
		local x = 0
		for i = at, at + k - 1 do
			local it = list[i]
			local w = math.floor(shapes[i] * h)
			-- (A full row's last picture takes what's left, so the row ends
			-- on the tray's edge.)
			if i == at + k - 1 and spans then
				w = math.max(1, math.floor(inner - x))
			end
			local crop: "none" | "top" | "small" = "none"
			local pw, ph = nil, nil
			if it.w and it.h then
				if it.w <= w and it.h <= h then
					crop, pw, ph = "small", it.w, it.h
				elseif it.w / it.h < 0.6 then
					crop = "top"
				end
			end
			places[it.src] = { x = x, y = y, w = w, h = h, crop = crop, pw = pw, ph = ph }
			x += w + gap
		end
		widest = math.max(widest, x - gap)
		y += h + gap
		at += k
	end
	return places, widest, math.max(0, y - gap)
end

-- The picture to the clipboard (`image.copy`, decoded off the main thread
-- by the host), the path, Finder: as the lightbox's.
local function copy_image(file: string)
	local ok, promise = pcall(ui.call, "image.copy", { path = file })
	if ok and promise then
		promise:after(function()
			toast.show { title = locale.s("ds.lightbox.image_copied"), tone = "success" }
		end, function(err: any)
			toast.show { title = locale.s("ds.lightbox.copy_failed"), body = tostring(err), tone = "danger" }
		end)
	end
end

local function copy_path(path: string)
	ui.clipboard.write(path)
	toast.show { title = locale.s("ds.lightbox.path_copied"), tone = "success" }
end

local function reveal(path: string)
	local ok, promise = pcall(ui.call, "image.reveal", { path = path })
	if ok and promise then
		promise:after(nil, function(err: any)
			toast.show { title = locale.s("ds.lightbox.reveal_failed"), body = tostring(err), tone = "danger" }
		end)
	end
end

-- What a picture is now: as its reader says, or as one that can't be
-- shown (not one to draw, or its kept copy gone).
local function now_of(pictures: Pictures, src: string): Picture
	local p = pictures.resolve(src) or { state = "failed", error = locale.s("ds.reply_picture.not_a_picture") }
	local broken = if p.state == "ready" and p.src then BROKEN:get()[p.src] else nil
	if broken then
		return { state = "failed", error = if broken ~= "" then broken else locale.s("ds.reply_picture.gone"), path = p.path, here = p.here }
	end
	return p
end

-- Asked for once as it's drawn, and again when a failed read is due.
local function follow(pictures: Pictures, src: string)
	ui.effect(function()
		pictures.fetch(src)
	end)
	ui.effect(function()
		local p = pictures.resolve(src)
		if p and p.state == "failed" and p.retry then
			ui.after(math.max(0, p.retry - ui.now()), function()
				pictures.fetch(src)
			end)
		end
	end)
end

-- The lightbox, opened on `src` among `set`'s pictures that are shown.
local function open_among(pictures: Pictures, set: () -> { Entry }, src: string, group: string)
	local items: { Lightbox.Item } = {}
	local at = 1
	for _, e in set() do
		local p = pictures.resolve(e.src)
		if p and p.state == "ready" and p.src and not BROKEN:peek()[p.src] then
			local w, h = size_of(p.src)
			table.insert(items, {
				src = p.src,
				name = name_of(e),
				key = e.src,
				path = if p.here then p.path else nil,
				w = w,
				h = h,
			})
			if e.src == src then
				at = #items
			end
		end
	end
	if #items > 0 then
		Lightbox.open(items, at, group)
	end
end

-- One of the controls at the top right: a small glass square.
local function control_button(icon: Icon.Name, label: any, radius: number, on_click: () -> (), focus: (boolean) -> ()): Node
	return ui.div(glass.style({
		rounded = ui.raw(radius, "concentric: the frame's corner less the controls' inset"),
		IconButton {
			icon = icon,
			label = label,
			size = "lg",
			rounded = ui.raw(radius, "fills its glass square"),
			on_click = on_click,
			on_focus = function()
				focus(true)
			end,
			on_blur = function()
				focus(false)
			end,
		},
	}, "island"))
end

-- A picture's frame, with what's in it while it comes, once it's here, and
-- its controls. `size()` is the frame's (`fixed`: the caller places it, as
-- a run's tile; otherwise it scales down with the column, keeping its
-- shape). `crop()` says how the picture sits.
type FrameProps = {
	pictures: Pictures,
	entry: Entry,
	group: string,
	set: () -> { Entry },
	size: () -> (number, number),
	crop: () -> Fit,
	fixed: boolean,
	-- (A run's tile: its place.)
	place: (() -> Place)?,
}

local Frame = ui.component(function(props: FrameProps): Node
	local m = measures()
	local pictures, e = props.pictures, props.entry
	local src = e.src
	local name = name_of(e)
	follow(pictures, src)
	local function now(): Picture
		return now_of(pictures, src)
	end
	local function file(): string
		local p = now()
		return if p.state == "ready" then p.src or "" else ""
	end
	local function ready(): boolean
		return now().state == "ready"
	end
	local hovered = ui.use_state(false)
	local focused = ui.use_state(0)
	local box = ui.use_state({ w = 0, h = 0 })
	local function focus(on: boolean)
		focused:set(math.max(0, focused:peek() + (if on then 1 else -1)))
	end
	-- Its controls show over a frame big enough to hold them.
	local function roomy(): boolean
		local b = box:get()
		return b.w >= 120 and b.h >= 72
	end
	local function controls_up(): boolean
		return ready() and roomy() and (hovered:get() or focused:get() > 0)
	end
	local inner = m.R - m.CONTROL
	local function open()
		if ready() then
			open_among(pictures, props.set, src, props.group)
		end
	end

	-- Its file couldn't be drawn: the picture is one that can't be shown.
	local function broke(why: any)
		local f = file()
		if f ~= "" and not BROKEN:peek()[f] then
			BROKEN:set_key(f, if string.find(tostring(why), "No such file", 1, true) then "" else M.plain(tostring(why)) or "")
		end
	end
	-- Its pixels' size as drawn: the frame and the lightbox follow it.
	local function landed(sz: { w: number, h: number })
		decoded(file(), sz)
	end
	-- The picture, as `crop` says (a real swap: which way it sits).
	local alpha = Motion.tween(ui.computed(function(): number
		return if ready() then 1 else 0
	end), { duration = 200, ease = Motion.EASE })
	local picture = ui.presence(function(): string
		return props.crop().crop
	end, function(crop: string): Node
		if crop == "top" then
			-- A long page: its top, fading out at the foot.
			return ui.div {
				w_full = true,
				h_full = true,
				overflow = "hidden",
				fade = { 0, 56 },
				ui.image {
					src = file,
					aria_hidden = true,
					on_error = broke,
					on_load = landed,
					w_full = true,
					aspect = function(): number
						local w, h = size_of(file())
						return if w and h then w / h else 1
					end,
					-- (Covered, never stretched, should its box and its
					-- pixels ever disagree.)
					fit = "cover",
				},
			}
		elseif crop == "small" then
			return ui.column {
				w_full = true,
				h_full = true,
				items = "center",
				justify = "center",
				ui.image {
					src = file,
					aria_hidden = true,
					w = function(): number
						return props.crop().pw or 0
					end,
					h = function(): number
						return props.crop().ph or 0
					end,
					fit = "contain",
					on_error = broke,
					on_load = landed,
				},
			}
		end
		return ui.image { src = file, aria_hidden = true, w_full = true, h_full = true, fit = "cover", on_error = broke, on_load = landed }
	end, { w_full = true, h_full = true, opacity = alpha })

	local frame: { [any]: any } = {
		relative = true,
		rounded = "$zeron.prose.picture.radius",
		overflow = "hidden",
		border = 1,
		border_over = true,
		border_color = "$border",
		bg = "$hover",
		focusable = true,
		role = "button",
		label = function(): string
			return if ready()
				then locale.s("ds.reply_picture.open_named", { name = name })
				else locale.s("ds.reply_picture.loading_named", { name = name })
		end,
		cursor = function(): string
			return if ready() then "zoom_in" else "default"
		end,
		focus = control.ring,
		on_click = open,
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		on_focus = function()
			focus(true)
		end,
		on_blur = function()
			focus(false)
		end,
		on_resize = function(s: { w: number, h: number })
			box:set({ w = s.w, h = s.h })
		end,
		picture,
		-- Coming: the frame alone, a faint picture glyph in its middle.
		ui.show(
			function(): boolean
				return not ready()
			end,
			ui.column {
				absolute = true,
				left = 0,
				top = 0,
				right = 0,
				bottom = 0,
				items = "center",
				justify = "center",
				color = "$text.tertiary",
				opacity = 0.6,
				pointer_events = "none",
				Icon { name = "gallery", size = 20 },
			}
		),
		-- Its controls, top right.
		ui.row {
			absolute = true,
			top = m.CONTROL,
			right = m.CONTROL,
			gap = "$space.1",
			opacity = Motion.tween(ui.computed(function(): number
				return if controls_up() then 1 else 0
			end), { duration = 140, ease = Motion.EASE }),
			pointer_events = function(): string
				return if controls_up() then "auto" else "none"
			end,
			ui.show(function(): boolean
				local p = now()
				return p.here == true and p.path ~= nil
			end, control_button("folder", locale.t("ds.reply_picture.show_in_finder"), inner, function()
				local p = now()
				if p.path then
					reveal(p.path)
				end
			end, focus)),
			control_button("copy", locale.t("ds.reply_picture.copy_image"), inner, function()
				local f = file()
				if f ~= "" then
					copy_image(f)
				end
			end, focus),
		},
	}
	if props.fixed then
		local place = props.place :: () -> Place
		frame.w = function(): number
			return place().w
		end
		frame.h = function(): number
			return place().h
		end
	else
		-- (Sized by the box round it, which scales down with the column.)
		frame.w_full = true
		frame.h_full = true
	end
	-- The lightbox's thumbnail: it grows from here and goes back here.
	for k, v in Lightbox.source(props.group, src, m.R) do
		frame[k] = v
	end
	local open_menu = ui.use_state(false)
	return ContextMenu {
		open = open_menu,
		ring = "$zeron.prose.picture.radius",
		label = locale.t("ds.reply_picture.actions"),
		-- Alone: its own shape, as wide as it is (no wider than the
		-- column, its height following), easing to a new size.
		max_w = if props.fixed then nil else "100%",
		w = if props.fixed then nil else Motion.tween(ui.computed(function(): number
			local w = props.size()
			return w
		end), { duration = 200, ease = Motion.EASE }),
		aspect = if props.fixed then nil else Motion.tween(ui.computed(function(): number
			local w, h = props.size()
			return w / h
		end), { duration = 200, ease = Motion.EASE }),
		items = function(): any
			local p = now()
			if p.state ~= "ready" then
				return nil
			end
			local entries: { any } = {
				{ label = locale.t("ds.reply_picture.open"), icon = "expand-arrows", on_select = open },
				{ separator = true },
				{ label = locale.t("ds.reply_picture.copy_image_menu"), icon = "copy", on_select = function()
					copy_image(p.src or "")
				end },
			}
			if p.path then
				table.insert(entries, { label = locale.t("ds.reply_picture.copy_path"), on_select = function()
					copy_path(p.path :: string)
				end })
			end
			if p.here and p.path then
				table.insert(entries, { label = locale.t("ds.reply_picture.show_in_finder"), icon = "folder", on_select = function()
					reveal(p.path :: string)
				end })
			end
			return entries
		end,
		ui.div(frame),
	}
end)

-- What a reason says, plainly: its first line, without where in a script
-- or a file it was raised (`visual:2: `, `/a/b.rs:10:4: `) or a stack;
-- nil when nothing plain is left.
function M.plain(reason: string?): string?
	if type(reason) ~= "string" then
		return nil
	end
	local line = string.match(reason, "^%s*([^\n]*)") or ""
	-- (Every `place:line:` or `place:line:col:` in it.)
	line = string.gsub(line, "[%w_%./%-@%[%]\"]+:%d+:%d*:?%s*", "")
	line = string.gsub(line, "^[Ee]rror:%s*", "")
	line = string.gsub(line, "^runtime error:%s*", "")
	line = string.match(line, "^%s*(.-)%s*$") or ""
	-- (A script's own fault, in a script's words: none of the reader's.)
	local lower = string.lower(line)
	for _, said in { "traceback", "attempt to", " nil", "stack", "panicked", "::" } do
		if string.find(lower, said, 1, true) then
			return nil
		end
	end
	if line == "" then
		return nil
	end
	if #line > 140 then
		line = string.sub(line, 1, 139) .. "…"
	end
	return line
end

export type LineProps = {
	-- Its glyph and its words (short, human: "Couldn't show this image").
	icon: Icon.Name?,
	words: string | () -> string,
	-- Why, in plain words (`M.plain`): its tooltip.
	reason: string?,
	-- Where it's a link (a reply's picture that can't be shown opens its
	-- path, as Zeron keeps it a link): what a click does, and its menu.
	on_click: (() -> ())?,
	context_menu: ((e: PointerEvent) -> { Node }?)?,
}

-- Something a reply meant to show that can't be: a quiet caption line in
-- the reply's flow, a small glyph and a few words in the tertiary ink,
-- the prose's small size, no frame or chrome; why, on hover.
function M.Line(props: LineProps): Node
	local click = props.on_click
	local words = props.words
	return ui.row {
		class = "md-image-error",
		self = "start",
		max_w = "100%",
		min_h = "$zeron.prose.small.line",
		gap = "$space.2",
		items = "center",
		color = "$text.tertiary",
		tooltip = props.reason,
		role = if click then "link" else nil,
		label = if type(words) == "string" then words else nil,
		cursor = if click then "pointer" else nil,
		focusable = if click then true else nil,
		focus = if click then control.ring else nil,
		hover = if click then { color = "$text.secondary" } else nil,
		on_click = click,
		context_menu = props.context_menu,
		Icon { name = props.icon or "gallery", size = 16 },
		ui.text {
			words,
			literal = true,
			truncate = true,
			text_size = "$zeron.prose.small.size",
			line_height = "$zeron.prose.small.line",
			underline = if click then true else nil,
			underline_offset = if click then 4 else nil,
		},
	}
end

-- A reply's picture that can't be shown: its line, a link to its path
-- where links open. Read again when its retry is due.
local Missing = ui.component(function(props: { pictures: Pictures, entry: Entry, open_link: ((target: string) -> ())?, link_menu: ((target: string) -> { Links.Entry }?)? }): Node
	local e = props.entry
	local open = props.open_link
	local asked = props.link_menu
	follow(props.pictures, e.src)
	return M.Line {
		words = locale.s("ds.reply_picture.not_shown", { name = name_of(e) }),
		reason = M.plain(now_of(props.pictures, e.src).error),
		on_click = if open
			then function()
				open(e.src)
			end
			else nil,
		context_menu = if open or asked
			then Links.menu_for(e.src, function(): { Links.Entry }?
				local own = if asked then asked(e.src) else nil
				return own or Links.other(e.src, if open
					then function()
						open(e.src)
					end
					else nil)
			end)
			else nil,
	}
end)

-- One picture alone.
function M.Single(props: Props): Node
	return ui.component(function(): Node
		local pictures = props.pictures
		local e: Entry = { src = props.src, alt = props.alt }
		local function fit(): Fit
			local m = measures()
			local p = now_of(pictures, e.src)
			local w, h = size_of(if p.state == "ready" then p.src else nil)
			if not w or not h then
				return { w = m.W, h = math.min(m.H, m.W / m.PLACE), crop = "none" }
			end
			return M.fit(w :: number, h :: number, m.W, m.H)
		end
		return ui.div {
			class = "md-image",
			min_w = 0,
			max_w = "100%",
			w_full = true,
			ui.presence(function(): string
				return if now_of(pictures, e.src).state == "failed" then "failed" else "frame"
			end, function(kind: string): Node
				if kind == "failed" then
					return Missing { pictures = pictures, entry = e, open_link = props.open_link, link_menu = props.link_menu }
				end
				return Frame {
					pictures = pictures,
					entry = e,
					group = props.group,
					set = props.set,
					size = function(): (number, number)
						local f = fit()
						return f.w, f.h
					end,
					crop = fit,
					fixed = false,
				}
			end),
		}
	end)({})
end

-- Pictures in a row, on a tray.
function M.Run(props: RunProps): Node
	return ui.component(function(): Node
		local m = measures()
		local pictures = props.pictures
		local entries = ui.computed(props.items)
		local column = ui.use_state(m.W)
		local function shown(): { Entry }
			local out = {}
			for _, e in entries:get() do
				if now_of(pictures, e.src).state ~= "failed" then
					table.insert(out, e)
				end
			end
			return out
		end
		local function missing(): { Entry }
			local out = {}
			for _, e in entries:get() do
				if now_of(pictures, e.src).state == "failed" then
					table.insert(out, e)
				end
			end
			return out
		end
		local laid = ui.computed(function(): any
			local list = {}
			for _, e in shown() do
				local p = now_of(pictures, e.src)
				local w, h = size_of(if p.state == "ready" then p.src else nil)
				table.insert(list, { src = e.src, w = w, h = h })
			end
			local inner = math.min(column:get(), m.W) - m.INSET * 2
			local places, w, h = M.layout(list, math.max(inner, 1))
			return { places = places, w = w, h = h }
		end)
		local tiles = ui.computed(shown)
		return ui.column {
			class = "md-image",
			w_full = true,
			min_w = 0,
			gap = "$space.2",
			on_resize = function(s: { w: number, h: number })
				if math.abs(column:peek() - s.w) >= 0.5 then
					column:set(s.w)
				end
			end,
			ui.show(
				function(): boolean
					return #tiles:get() > 0
				end,
				ui.div {
					relative = true,
					rounded = ui.raw(m.R + m.INSET, "concentric: the pictures' corner plus the tray's inset"),
					bg = "$zeron.prose.code.bg",
					w = function(): number
						return laid:get().w + m.INSET * 2
					end,
					h = function(): number
						return laid:get().h + m.INSET * 2
					end,
					max_w = "100%",
					ui.each(tiles, function(e: Entry): Node
						local function place(): Place
							return laid:get().places[e.src] or { x = 0, y = 0, w = 1, h = 1, crop = "none" }
						end
						return ui.div {
							absolute = true,
							left = function(): number
								return place().x + m.INSET
							end,
							top = function(): number
								return place().y + m.INSET
							end,
							Frame {
							pictures = pictures,
							entry = e,
							group = props.group,
							set = props.set,
							fixed = true,
							place = place,
							size = function(): (number, number)
								local p = laid:get().places[e.src]
								return if p then p.w else 1, if p then p.h else 1
							end,
							crop = function(): Fit
								local p = laid:get().places[e.src]
								return if p then { w = p.w, h = p.h, crop = p.crop, pw = p.pw, ph = p.ph } else { w = 1, h = 1, crop = "none" }
							end,
							},
						}
					end, {
						key = function(e: Entry): string
							return e.src
						end,
						absolute = true,
						left = 0,
						top = 0,
						right = 0,
						bottom = 0,
					}),
				}
			),
			ui.each(ui.computed(missing), function(e: Entry): Node
				return Missing { pictures = pictures, entry = e, open_link = props.open_link, link_menu = props.link_menu }
			end, {
				key = function(e: Entry): string
					return e.src
				end,
				gap = "$space.2",
			}),
		}
	end)({})
end

return M

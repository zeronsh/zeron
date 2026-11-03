-- The lightbox: a picture looked at whole, over the window. Zeron's viewer
-- (crates/ui image_viewer.rs, attachments.rs `lightbox`): the picture fitted,
-- never past its own size, over a scrim; a pinch or ⌘-scroll zooms about
-- the pointer, a drag pans, its edges never come inside the window's; a
-- click outside it or Escape closes it and focus goes back where it was.
--
-- It opens from the thumbnail that was clicked: the thumbnail lifts out of
-- its place and grows into the fitted picture, its crop opening out to the
-- whole picture and its corner squaring as it goes, while the scrim comes
-- up. Closing, it goes back down into wherever that thumbnail is now, or,
-- when the thumbnail has scrolled away, settles back and fades. The picture
-- is laid out at its fitted size from the first frame and the motion
-- scales it, so it is decoded and uploaded once, at the size it ends at.
--
-- Zoom is the files viewer's (ds/file_viewer, an image): ⌘+ ⌘− walk the
-- same steps, ⌘0 fits, ⌘1 is its own size (one pixel a point), a double
-- click toggles fitted and its own size about the pointer, a pinch or
-- ⌘-scroll zooms about the pointer, scrolling pans, and here a drag pans
-- too, carrying on as it is let go. While a gesture runs the picture is
-- scaled as drawn; once it rests it is laid out at the size it shows, so
-- it is as sharp as the file is, and from its own size up its pixels show
-- square (no soft upscale). ← → go between the pictures of one set (a
-- message's, the composer's), ⌘C copies the picture, and its controls (a
-- small glass island at the foot) come up as the pointer moves and go
-- again when it rests.
--
-- One `Lightbox.Host()` per window draws it. A thumbnail opens it:
--
--   Lightbox.Thumb { group = "message-12", index = 2, items = images,
--     radius = 10, w = 120, h = 84, ui.image { … } }
--
-- or `Lightbox.open(items, index, group)` from anywhere. An item is
-- `{ src, name, path?, w?, h? }`: `src` what `ui.image` shows, `path` the
-- file on this Mac (Copy path, Reveal), `w`/`h` its size in pixels if known.

local Icon = require("@self/icon")
local IconButton = require("@self/icon_button").IconButton
local glass = require("@self/glass")
local control = require("@self/control")
local Motion = require("@self/motion")
local locale = require("@self/locale")

local M = {}

export type Item = {
	src: string,
	name: string,
	path: string?,
	w: number?,
	h: number?,
	-- Which thumbnail it is in its group (a chip's id), where places move;
	-- else its place in the set.
	key: string?,
}

type Shown = { items: { Item }, index: number, group: string? }
type Rect = { x: number, y: number, w: number, h: number }

-- What is open (plain data, kept across a reload, so a reload with it open
-- shows it where it was, with no motion): the set, which of it, and the
-- thumbnails' group.
local OPEN = ui.state("open", nil :: Shown?)
-- The thumbnail lifted out while the picture is up: its key.
local LIFTED = ui.state("lifted", nil :: string?)

-- Thumbnails by key (`group:index`): their elements, whether any of them
-- shows, and their corner. Kept here, not in state: they are re-registered
-- as each mounts, a reload included.
local thumbs: { [string]: any } = {}
local seen: { [string]: boolean } = {}
local corners: { [string]: number } = {}
-- Pictures' sizes in pixels, by `src`, as they decoded (`on_load`).
local sizes: { [string]: { w: number, h: number } } = {}

-- Zoom: the steps ⌘+ and ⌘− walk, and the furthest out and in (the files
-- viewer's).
local STEPS = { 0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 1, 1.25, 1.5, 2, 3, 4, 6, 8, 12, 16 }
local MIN_ZOOM, MAX_ZOOM = 0.02, 32
-- How far a press moves before it is a drag (zgx's drag threshold).
local DRAG = 4
-- A gesture's scale is laid out once it has rested this long.
local SETTLE = 140
-- The controls go this long after the pointer rests.
local IDLE = 1600

-- Motion: in and out, and a zoom's step, ds/motion's snappy (moving from
-- the press's own frame, fast at first, no give); a picture swapped, or any
-- of it with reduced motion, a short fade.
local FADE = 160
-- A fade's length now: with motion reduced, ds/motion's crossfade at most.
local function fade_ms(): number
	return if Motion.reduced_now() then math.min(FADE, Motion.FADE) else FADE
end
-- Every duration times this: a test's slow motion (`zgx inspect set
-- 'ds/lightbox.luau::slow' 10`), 1 otherwise.
local SLOW = ui.state("slow", 1)
local function ms(d: number): number
	return d * math.max(1, SLOW:peek())
end

-- The window's size in points: `ui.window.size`, followed when `track`;
-- 1 by 1 where a host gives none (a headless test, a host that doesn't
-- feed it), so nothing here divides by nothing or reads a missing field.
-- (A plugin's scripts read it too.)
local function window_size(track: boolean?): { width: number, height: number }
	local window = (ui :: any).window
	local state = if type(window) == "table" or type(window) == "userdata" then window.size else nil
	local ok, v = pcall(function(): any
		if state == nil then
			return nil
		end
		return if track then state:get() else state:peek()
	end)
	local w = if ok and type(v) == "table" and type(v.width) == "number" then v.width else 0
	local h = if ok and type(v) == "table" and type(v.height) == "number" then v.height else 0
	return { width = math.max(1, w), height = math.max(1, h) }
end

local function n(token: string, fallback: number): number
	local v = ui.theme.get(token)
	return if type(v) == "number" then v else fallback
end

-- A thumbnail's key: its group and the item's `key` (else its place).
local function key_of(group: string?, i: number, items: { Item }?): string?
	if not group then
		return nil
	end
	local it = if items then items[i] else nil
	return `{group}:{if it and it.key then it.key else tostring(i)}`
end

local function lerp(a: number, b: number, t: number): number
	return a + (b - a) * t
end

-- A picture of an element as the window last showed it (`el:snapshot`): a
-- test's host has none, and puts its own here.
M.snapshot = function(el: any, max_width: number): string?
	return el:snapshot(max_width)
end

-- Open `items` at `index`; `group` names the thumbnails it opens from
-- (`Lightbox.Thumb`'s), so it grows from and goes back to them.
function M.open(items: { Item }, index: number, group: string?)
	if #items == 0 then
		return
	end
	index = math.clamp(index, 1, #items)
	-- Its thumbnail takes focus first, so closing gives it back there (a
	-- click focuses nothing of itself).
	local key = key_of(group, index, items)
	local el = if key then thumbs[key] else nil
	if el and el.mounted then
		el:focus()
	end
	OPEN:set({ items = items, index = index, group = group })
end

-- Close it (it goes back to its thumbnail).
function M.close()
	OPEN:set(nil)
end

-- Whether it is open.
function M.is_open(): boolean
	return OPEN:get() ~= nil
end

-- The picture to the clipboard, through the host (`image.copy`, off the
-- main thread); where the host has none (the gallery under `zgx run`),
-- nothing.
local function copy_image(item: Item)
	local path = item.path or item.src
	local ok, promise = pcall(ui.call, "image.copy", { path = path })
	if ok and promise then
		promise:after(function()
			ui.toast { title = locale.s("ds.lightbox.image_copied"), kind = "success" }
		end, function(err: any)
			ui.toast { title = locale.s("ds.lightbox.copy_failed"), body = tostring(err), kind = "error" }
		end)
	end
end

local function copy_path(item: Item)
	ui.clipboard.write(item.path or item.src)
	ui.toast { title = locale.s("ds.lightbox.path_copied"), kind = "success" }
end

local function reveal(item: Item)
	local ok, promise = pcall(ui.call, "image.reveal", { path = item.path or item.src })
	if ok and promise then
		promise:after(nil, function(err: any)
			ui.toast { title = locale.s("ds.lightbox.reveal_failed"), body = tostring(err), kind = "error" }
		end)
	end
end

-- The stage: the picture over the scrim, its controls, and every gesture.
-- Built each time it opens (its zoom new); `shown` is the set, kept by the
-- host while it closes.
type Stage = {
	shown: State<Shown?>,
	-- 0 to 1: the scrim and the controls coming up (and the picture, where
	-- it fades rather than moves).
	lift: State<number>,
	-- The picture's own fade and its settle while it fades (1 = in place).
	alpha: State<number>,
	settle: State<number>,
	-- 0 to 1, from the thumbnail to in place, while `morphing`.
	t: State<number>,
	morphing: State<boolean>,
	-- Going: the controls go with the scrim.
	leaving: State<boolean>,
	-- The thumbnail's picture as it showed (`el:snapshot()`): under the
	-- picture while it grows, standing in until the picture has decoded.
	snap: State<string>,
	-- The thumbnail's rect and corner, read as it opens and again as it
	-- closes.
	from: { rect: Rect?, radius: number },
	-- Hands the host how it goes back: the shown picture's rect now.
	register: (api: { rect: () -> Rect?, ready: () -> boolean, gone: () -> boolean }) -> (),
	-- Ready: the picture is measured and laid out; the host starts its
	-- motion.
	on_ready: () -> (),
	-- Close.
	close: () -> (),
}

-- The stage's view, for tests (`Lightbox._view()`): its scale, middle,
-- whether fitted, the picture's size and the window's.
local probe: (() -> any)? = nil
function M._view(): any
	return if probe then (probe :: () -> any)() else nil
end

local StageView = ui.component(function(st: Stage): Node
	local function item(): Item?
		local s = st.shown:get()
		return if s then s.items[s.index] else nil
	end
	-- The picture's size in pixels, the truest first: as it decoded
	-- (`on_load`), then its file's header now (`ui.image_size`, no decode;
	-- never kept, as a path can be written over with another picture), and
	-- only then the size its item says. The picture is laid out at this
	-- size and only ever scaled evenly, so it is never stretched.
	local measured = ui.use_state(0)
	local headers: { [string]: { w: number, h: number } | false } = {}
	local function natural(): { w: number, h: number }?
		measured:get()
		local it = item()
		if not it then
			return nil
		end
		local s = sizes[it.src]
		if s then
			return s
		end
		-- (Read once while it's open, not on every frame of a motion: this
		-- stage is built again each time it opens.)
		local hd = headers[it.src]
		if hd == nil then
			local ok, w, h = pcall(ui.image_size, it.src)
			hd = if ok and type(w) == "number" and type(h) == "number" and w > 0 and h > 0 then { w = w, h = h } else false
			headers[it.src] = hd
		end
		if hd then
			return hd
		end
		if it.w and it.h and it.w > 0 and it.h > 0 then
			return { w = it.w, h = it.h }
		end
		return nil
	end
	-- The view: the picture's scale, and where its middle sits in the
	-- window; plain (moved per gesture), `tick` redraws.
	local view = { s = 1, cx = 0, cy = 0, fitted = true }
	local tick = ui.use_state(0)
	local function redraw()
		tick:set(tick:peek() + 1)
	end
	-- The size the picture is laid out at (its scale at the last rest).
	local laid = ui.use_state(1)

	-- The room it is fitted in: the window less its inset, and the foot
	-- kept for the controls.
	local function room(): (number, number, number, number)
		local size = window_size(true)
		local inset = n("zeron.lightbox.inset", 40)
		local foot = n("zeron.lightbox.foot", 72)
		return inset, inset, math.max(1, size.width - inset * 2), math.max(1, size.height - inset - foot)
	end
	local function fit_scale(): number
		local nat = natural()
		if not nat then
			return 1
		end
		local _, _, w, h = room()
		return math.min(1, w / nat.w, h / nat.h)
	end
	-- Keeps the picture in view: on an axis it fits, at the room's middle
	-- (nudged in to show whole); larger than the window, its edges never
	-- inside the window's.
	local function clamp_view()
		local nat = natural()
		if not nat then
			return
		end
		local size = window_size(false)
		local x, y, w, h = room()
		local function axis(c: number, extent: number, span: number, home: number): number
			if extent <= span then
				return math.clamp(home, extent / 2, span - extent / 2)
			end
			return math.clamp(c, span - extent / 2, extent / 2)
		end
		view.cx = axis(view.cx, nat.w * view.s, size.width, x + w / 2)
		view.cy = axis(view.cy, nat.h * view.s, size.height, y + h / 2)
	end
	local function fit()
		view.s = fit_scale()
		view.fitted = true
		clamp_view()
	end

	-- Laying the picture out at the scale it shows, once a gesture rests
	-- (a drag or a pan never changes it).
	local settle_gen = 0
	local function settle_later()
		settle_gen += 1
		local gen = settle_gen
		ui.after(SETTLE, function()
			if gen == settle_gen and laid:peek() ~= view.s then
				laid:set(view.s)
				rested_at = ui.now()
			end
		end)
	end

	-- Where the picture shows, at rest: its box in the window.
	local function rest_rect(): Rect?
		local nat = natural()
		if not nat then
			return nil
		end
		local w, h = nat.w * view.s, nat.h * view.s
		return { x = view.cx - w / 2, y = view.cy - h / 2, w = w, h = h }
	end

	-- A zoom's motion (⌘+, a double click): eases the view there.
	local motion: any = nil
	-- Where a zoom's motion is going: the next ⌘+ or ⌘− steps on from
	-- there, so quick presses walk the steps.
	local heading: number? = nil
	local function stop_motion()
		if motion then
			motion:cancel()
			motion = nil
		end
		heading = nil
	end
	-- What the window showed of the picture as a zoom began, held under it
	-- while it zooms. Drawn larger than its pixels were decoded at, a
	-- picture from a file isn't drawn at all until its sharper pixels have
	-- decoded (zgx-asks/images, 4): every frame of a zoom in asks for more,
	-- so it blinked. This picture of it (a bitmap, never decoded again) is
	-- scaled with the zoom, exactly where the picture is, and shows wherever
	-- the picture isn't drawn yet; once it is, it covers it. Taken again at
	-- the next zoom once the picture has rested (a fresh, sharp one), else
	-- the one held is kept: it is still exactly where the picture is.
	-- (`s0`, `cx0`, `cy0`: the view it was taken at; `ix`… the picture's
	-- part of the window then; `w`, `h`: the window.)
	local stage_ref = ui.ref()
	-- (false when nothing is held: a state made with nil reads false.)
	local standin = ui.use_state(false :: any) :: State<{ src: string, item: string, s0: number, cx0: number, cy0: number, ix: number, iy: number, iw: number, ih: number, w: number, h: number } | false>
	local rested_at, zoomed_at = 0, 0
	local function hold()
		local it = item()
		if not it or st.morphing:peek() then
			return
		end
		local h = standin:peek()
		local now = ui.now()
		local busy = motion ~= nil or now - zoomed_at < SETTLE + 260 or now - rested_at < SETTLE + 260
		zoomed_at = now
		-- (Mid-zoom, or the picture's sharp pixels maybe not drawn yet: the
		-- one held stays, still exactly where the picture is.)
		if h and h.item == it.src and busy then
			return
		end
		local el = stage_ref:peek()
		local nat = natural()
		if not el or not el.mounted or not nat then
			return
		end
		local size = window_size(false)
		local shot = M.snapshot(el, math.ceil(size.width * 2))
		if not shot or shot == "" then
			return
		end
		local dw, dh = nat.w * view.s, nat.h * view.s
		local x0, y0 = math.max(0, view.cx - dw / 2), math.max(0, view.cy - dh / 2)
		local x1, y1 = math.min(size.width, view.cx + dw / 2), math.min(size.height, view.cy + dh / 2)
		if x1 <= x0 or y1 <= y0 then
			return
		end
		standin:set({
			src = shot,
			item = it.src,
			s0 = view.s,
			cx0 = view.cx,
			cy0 = view.cy,
			ix = x0,
			iy = y0,
			iw = x1 - x0,
			ih = y1 - y0,
			w = size.width,
			h = size.height,
		})
	end

	-- To scale `s`, keeping the window point (px, py) where it is.
	local function zoom_to(s: number, px: number?, py: number?, animate: boolean?)
		hold()
		stop_motion()
		s = math.clamp(s, math.min(MIN_ZOOM, fit_scale()), MAX_ZOOM)
		local size = window_size(false)
		local ax, ay = px or size.width / 2, py or size.height / 2
		local k = s / view.s
		local from = { s = view.s, cx = view.cx, cy = view.cy }
		view.s = s
		view.cx = ax - (ax - view.cx) * k
		view.cy = ay - (ay - view.cy) * k
		view.fitted = false
		clamp_view()
		if animate and not Motion.reduced_now() then
			local to = { s = view.s, cx = view.cx, cy = view.cy }
			local probe = { s = from.s, cx = from.cx, cy = from.cy }
			view.s, view.cx, view.cy = from.s, from.cx, from.cy
			local params: TableParams = {
				s = to.s,
				cx = to.cx,
				cy = to.cy,
				duration = ms(Motion.SNAPPY.zoom),
				ease = Motion.SNAPPY.ease,
				onUpdate = function()
					view.s, view.cx, view.cy = probe.s, probe.cx, probe.cy
					redraw()
				end,
				onComplete = function()
					motion = nil
					heading = nil
					settle_later()
				end,
			}
			motion = anime.animate(probe, params)
			heading = to.s
		else
			redraw()
			settle_later()
		end
	end
	local function fit_animated()
		hold()
		stop_motion()
		local from = { s = view.s, cx = view.cx, cy = view.cy }
		fit()
		local to = { s = view.s, cx = view.cx, cy = view.cy }
		if Motion.reduced_now() then
			redraw()
			settle_later()
			return
		end
		view.s, view.cx, view.cy = from.s, from.cx, from.cy
		local probe = { s = from.s, cx = from.cx, cy = from.cy }
		motion = anime.animate(probe, {
			s = to.s,
			cx = to.cx,
			cy = to.cy,
			duration = ms(Motion.SNAPPY.zoom),
			ease = Motion.SNAPPY.ease,
			onUpdate = function()
				view.s, view.cx, view.cy = probe.s, probe.cx, probe.cy
				redraw()
			end,
			onComplete = function()
				motion = nil
				view.fitted = true
				settle_later()
			end,
		})
	end
	local function step(dir: number)
		local s = heading or view.s
		local next_s = if dir > 0 then STEPS[#STEPS] else STEPS[1]
		if dir > 0 then
			for _, v in STEPS do
				if v > s + 0.001 then
					next_s = v
					break
				end
			end
		else
			for i = #STEPS, 1, -1 do
				if STEPS[i] < s - 0.001 then
					next_s = STEPS[i]
					break
				end
			end
		end
		zoom_to(next_s, nil, nil, true)
	end
	local function toggle_at(px: number?, py: number?)
		if view.fitted then
			zoom_to(1, px, py, true)
		else
			fit_animated()
		end
	end

	-- Measured (or known): fitted, laid out at that, then the host starts
	-- its motion. A reload finds it measured and at rest.
	local ready = false
	local function on_measured()
		if ready or not natural() then
			return
		end
		ready = true
		fit()
		laid:set(view.s)
		redraw()
		st.on_ready()
	end
	-- Another picture of the set: fitted, fading in where it is.
	local function on_swap()
		standin:set(false)
		stop_motion()
		fit()
		laid:set(view.s)
		redraw()
	end

	probe = function(): any
		local nat = natural()
		local size = window_size(false)
		-- (What's held under the picture, where it lands now, and the
		-- picture's part of the window it was taken from, mapped the same
		-- way: they are one rect.)
		local h = standin:peek()
		local it = item()
		local under = nil
		if h and it and h.item == it.src and nat then
			local r = view.s / h.s0
			-- (The picture's own corner, as the held one maps it, and as it
			-- is now.)
			local hx = view.cx + ((h.cx0 - nat.w * h.s0 / 2) - h.cx0) * r
			local hy = view.cy + ((h.cy0 - nat.h * h.s0 / 2) - h.cy0) * r
			under = {
				x = view.cx + (h.ix - h.cx0) * r,
				y = view.cy + (h.iy - h.cy0) * r,
				w = h.iw * r,
				h = h.ih * r,
				corner_x = hx,
				corner_y = hy,
				pic_x = view.cx - nat.w * view.s / 2,
				pic_y = view.cy - nat.h * view.s / 2,
			}
		end
		return { under = under, s = view.s, cx = view.cx, cy = view.cy, fitted = view.fitted, laid = laid:peek(), w = nat and nat.w, h = nat and nat.h, win_w = size.width, win_h = size.height, fit = fit_scale(), morphing = st.morphing:peek(), alpha = st.alpha:peek(), lift = st.lift:peek() }
	end
	-- Its file gone (a sent picture deleted) or not a picture: nothing to
	-- draw, and it says so in the picture's place (`ui.image`'s `on_error`).
	local broken = ui.use_state({} :: { [string]: boolean })
	local function gone(): boolean
		local it = item()
		return it ~= nil and broken:get()[it.src] == true
	end
	ui.effect(function()
		if gone() and not ready then
			ready = true
			st.on_ready()
		end
	end)
	-- Another of the set shown: fitted again, where it is.
	local swapped = false
	ui.effect(function()
		item()
		if swapped and ready then
			on_swap()
		end
		swapped = true
	end)
	-- A picture that says its size as it loads (one from the web).
	local function loaded(size: { w: number, h: number })
		local it = item()
		if not it or size.w <= 0 or size.h <= 0 then
			return
		end
		local had = natural()
		sizes[it.src] = { w = size.w, h = size.h }
		if had and had.w == size.w and had.h == size.h then
			return
		end
		sizes[it.src] = { w = size.w, h = size.h }
		measured:set(measured:peek() + 1)
		if ready then
			on_swap()
		end
		on_measured()
	end
	st.register {
		rect = function(): Rect?
			return rest_rect()
		end,
		ready = function(): boolean
			return ready
		end,
		gone = gone,
	}

	-- What is drawn, each frame of a motion: the clip (its box and corner)
	-- and the picture in it (its laid-out size, and the move and scale that
	-- place it).
	type Geo = { cx: number, cy: number, cw: number, ch: number, r: number, lw: number, lh: number, x: number, y: number, k: number, d: Rect, sn: Rect }
	local function geo(): Geo
		tick:get()
		local nat = natural() or { w = 1, h = 1 }
		local size = window_size(true)
		local lw, lh = nat.w * laid:get(), nat.h * laid:get()
		local rest = rest_rect() or { x = 0, y = 0, w = lw, h = lh }
		local settle = st.settle:get()
		if st.morphing:get() and st.from.rect then
			-- From the thumbnail: the clip between its box and the
			-- picture's, the picture covering the clip (the thumbnail's
			-- crop at 0, the whole picture at 1).
			local t = st.t:get()
			local a = st.from.rect :: Rect
			local cx, cy = lerp(a.x, rest.x, t), lerp(a.y, rest.y, t)
			local cw, ch = math.max(1, lerp(a.w, rest.w, t)), math.max(1, lerp(a.h, rest.h, t))
			local k = math.max(cw / lw, ch / lh)
			-- The thumbnail's crop of the picture, scaled with it.
			local k0 = math.max(a.w / lw, a.h / lh)
			local sw, sh = a.w / k0 * k, a.h / k0 * k
			return {
				sn = { x = cw / 2 - sw / 2, y = ch / 2 - sh / 2, w = sw, h = sh },
				cx = cx,
				cy = cy,
				cw = cw,
				ch = ch,
				r = math.max(0, lerp(st.from.radius, 0, t)),
				lw = lw,
				lh = lh,
				x = cw / 2 - lw / 2,
				y = ch / 2 - lh / 2,
				k = k,
				d = { x = cx, y = cy, w = cw, h = ch },
			}
		end
		-- At rest: the clip is the window; the picture at its view (settling
		-- toward its middle as it fades out).
		local k = rest.w / lw * settle
		local mx, my = rest.x + rest.w / 2, rest.y + rest.h / 2
		local dw, dh = lw * k, lh * k
		return {
			sn = { x = 0, y = 0, w = 1, h = 1 },
			cx = 0,
			cy = 0,
			cw = size.width,
			ch = size.height,
			r = 0,
			lw = lw,
			lh = lh,
			x = mx - lw / 2,
			y = my - lh / 2,
			k = k,
			d = { x = mx - dw / 2, y = my - dh / 2, w = dw, h = dh },
		}
	end
	-- Square pixels from its own size up, where they land on whole device
	-- pixels (2x and on) or are large enough to read as pixels (3x and on).
	local function rendering(): "auto" | "pixelated"
		tick:get()
		local s = view.s
		if s >= 2.99 or (s >= 0.999 and math.abs(s - math.floor(s + 0.5)) < 0.01) then
			return "pixelated"
		end
		return "auto"
	end

	-- The controls: awake while the pointer moves, or rests on them, or
	-- they hold focus.
	local awake = ui.use_state(true)
	local held = ui.use_state(false)
	local idle_gen = 0
	local function poke()
		if not awake:peek() then
			awake:set(true)
		end
		idle_gen += 1
		local gen = idle_gen
		ui.after(IDLE, function()
			if gen == idle_gen and not held:peek() then
				awake:set(false)
			end
		end)
	end
	poke()

	-- Pointer: a press (a drag pans, carrying on as it is let go), a click
	-- outside the picture closes, a double click toggles zoom.
	local press: { x: number, y: number, cx: number, cy: number, dragged: boolean, samples: { { number } } }? = nil
	local last_move: { number }? = nil
	local function inside(px: number, py: number): boolean
		local r = rest_rect()
		return r ~= nil and px >= r.x and px <= r.x + r.w and py >= r.y and py <= r.y + r.h
	end
	local function pannable(): boolean
		local r = rest_rect()
		local size = window_size(false)
		return r ~= nil and (r.w > size.width + 0.5 or r.h > size.height + 0.5)
	end
	local function fling(vx: number, vy: number)
		-- Carries on as a flick does, slowing to rest (about a third of a
		-- second's travel at the speed it was let go).
		local speed = math.sqrt(vx * vx + vy * vy)
		if speed < 0.15 or Motion.reduced_now() then
			return
		end
		stop_motion()
		local tau = 325
		local probe = { cx = view.cx, cy = view.cy }
		local from = { cx = view.cx, cy = view.cy }
		view.cx += vx * tau
		view.cy += vy * tau
		clamp_view()
		local to = { cx = view.cx, cy = view.cy }
		view.cx, view.cy = from.cx, from.cy
		if math.abs(to.cx - from.cx) < 0.5 and math.abs(to.cy - from.cy) < 0.5 then
			return
		end
		motion = anime.animate(probe, {
			cx = to.cx,
			cy = to.cy,
			duration = 650,
			ease = "outExpo",
			onUpdate = function()
				view.cx, view.cy = probe.cx, probe.cy
				redraw()
			end,
			onComplete = function()
				motion = nil
			end,
		})
	end

	local function shown_count(): number
		local s = st.shown:get()
		return if s then #s.items else 0
	end
	local function go(dir: number)
		local s = st.shown:peek()
		if not s or #s.items < 2 then
			return
		end
		local next_i = s.index + dir
		if next_i < 1 or next_i > #s.items then
			return
		end
		OPEN:set({ items = s.items, index = next_i, group = s.group })
	end

	local function zoom_label(): string
		tick:get()
		return `{math.floor(view.s * 100 + 0.5)}%`
	end

	local function on_key(e: KeyEvent): boolean?
		local it = item()
		if e.key == "ArrowLeft" and not e.cmd then
			go(-1)
		elseif e.key == "ArrowRight" and not e.cmd then
			go(1)
		elseif e.cmd and (e.key == "=" or e.key == "+") then
			step(1)
		elseif e.cmd and e.key == "-" then
			step(-1)
		elseif e.cmd and e.key == "0" then
			fit_animated()
		elseif e.cmd and e.key == "1" then
			zoom_to(1, nil, nil, true)
		elseif e.cmd and e.key == "c" and it then
			copy_image(it)
		elseif e.key == "Escape" then
			-- (Here, not the modal's: it would let focus go at once, while the
			-- thumbnail it goes back to is still lifted out.)
			st.close()
		elseif e.key == " " and not e.editing then
			-- (Space: Quick Look's way out.)
			st.close()
		else
			return nil
		end
		return true
	end

	local island_props: ContainerProps = {
		absolute = true,
		bottom = n("zeron.lightbox.island_inset", 16),
		self = "center",
		h = n("zeron.island.control", 24) + n("zeron.island.pad", 4) * 2,
		px = "$zeron.island.pad",
		gap = "$zeron.island.gap",
		items = "center",
		role = "toolbar",
		label = locale.t("ds.lightbox.image"),
		opacity = Motion.tween(ui.computed(function(): number
			return if awake:get() or held:get() then 1 else 0
		end), { duration = 180 }),
		on_hover = function(on: boolean)
			held:set(on)
			if not on then
				poke()
			end
		end,
	}
	glass.style(island_props, "island")
	local function divider(): Node
		return ui.div { w = 1, h = 14, mx = "$zeron.space.hair", bg = "$border.subtle", shrink = 0 }
	end
	-- (`key`: the name's message; a tooltip is a string, read as it's built.)
	local function button(icon: Icon.Name, key: string, keys: string?, on_click: () -> (), disabled: (() -> boolean)?): Node
		local label = locale.s(key)
		return IconButton {
			icon = icon,
			label = locale.t(key),
			tooltip = if keys then `{label}  {ui.shortcut_text(keys)}` else label,
			tooltip_side = "top",
			size = "md",
			on_click = on_click,
			disabled = disabled,
			on_focus = function()
				held:set(true)
			end,
			on_blur = function()
				held:set(false)
				poke()
			end,
		}
	end
	table.insert(island_props, ui.text {
		function(): string
			local it = item()
			return if it then it.name else ""
		end,
		literal = true,
		truncate = true,
		max_w = 220,
		px = "$space.2",
		text_size = "$font.size.sm",
		weight = "$font.weight.medium",
		color = "$text.primary",
	})
	table.insert(
		island_props,
		ui.show(
			function(): boolean
				return shown_count() > 1
			end,
			ui.row {
				gap = "$zeron.island.gap",
				items = "center",
				divider(),
				button("alt-arrow-left", "ds.lightbox.previous", "left", function()
					go(-1)
				end, function(): boolean
					local s = st.shown:get()
					return s == nil or s.index <= 1
				end),
				ui.text {
					function(): string
						local s = st.shown:get()
						return if s then locale.s("ds.lightbox.position", { i = tostring(s.index), n = tostring(#s.items) }) else ""
					end,
					tabular = true,
					text_size = "$font.size.sm",
					color = "$text.secondary",
					px = "$space.1",
				},
				button("alt-arrow-right", "ds.lightbox.next", "right", function()
					go(1)
				end, function(): boolean
					local s = st.shown:get()
					return s == nil or s.index >= #s.items
				end),
			}
		)
	)
	table.insert(island_props, divider())
	table.insert(island_props, button("minus", "ds.lightbox.zoom_out", "cmd+-", function()
		step(-1)
	end))
	table.insert(island_props, ui.button {
		label = locale.t("ds.lightbox.fit_or_own"),
		tooltip = locale.s("ds.lightbox.fit_tip", { fit = ui.shortcut_text("cmd+0"), actual = ui.shortcut_text("cmd+1") }),
		tooltip_placement = { side = "top", offset = 6 },
		h = "$zeron.island.control",
		min_w = 48,
		px = "$space.1",
		justify = "center",
		rounded = "$zeron.icon_button.radius",
		bg = "#00000000",
		color = "$text.secondary",
		hover = { bg = "$hover", color = "$text.primary" },
		active = { bg = "$pressed", color = "$text.primary" },
		focus = control.ring,
		cursor = "pointer",
		on_click = function()
			toggle_at(nil, nil)
		end,
		on_focus = function()
			held:set(true)
		end,
		on_blur = function()
			held:set(false)
			poke()
		end,
		ui.text { zoom_label, tabular = true, text_size = "$font.size.sm" },
	})
	table.insert(island_props, button("plus", "ds.lightbox.zoom_in", "cmd+=", function()
		step(1)
	end))
	table.insert(island_props, divider())
	table.insert(island_props, button("copy", "ds.lightbox.copy_image", "cmd+c", function()
		local it = item()
		if it then
			copy_image(it)
		end
	end))
	table.insert(island_props, button("close", "common.close", "escape", function()
		st.close()
	end))

	return ui.overlay {
		w_full = true,
		h_full = true,
		relative = true,
		focusable = true,
		label = function(): string
			local it = item()
			return if it then locale.s("ds.lightbox.image_named", { name = it.name }) else locale.s("ds.lightbox.image")
		end,
		focus = { border = 0 },
		on_key = on_key,
		on_mount = function(el)
			el:focus()
			-- (A picture whose file has gone is never measured: it comes up as
			-- it is, saying so.)
			if gone() and not ready then
				ready = true
				st.on_ready()
			end
		end,
		on_resize = function()
			-- (Its first layout: the picture's size is known from its file,
			-- so it's fitted and the motion starts.)
			if not ready then
				on_measured()
				return
			end
			if view.fitted then
				fit()
				laid:set(view.s)
			else
				clamp_view()
			end
			redraw()
		end,
		context_menu = function(): { Node }?
			local it = item()
			if not it then
				return nil
			end
			local items: { Node } = {
				ui.menu_item {
					label = locale.t("ds.lightbox.copy_image_menu"),
					shortcut = "cmd+c",
					on_click = function()
						copy_image(it)
					end,
				},
				ui.menu_item {
					label = locale.t("ds.lightbox.copy_path"),
					on_click = function()
						copy_path(it)
					end,
				},
			}
			if it.path then
				table.insert(items, ui.menu_item {
					label = locale.t("ds.lightbox.show_in_finder"),
					on_click = function()
						reveal(it)
					end,
				})
			end
			return items
		end,
		-- The scrim: the whole window, and every gesture's.
		ui.div {
			absolute = true,
			left = 0,
			top = 0,
			right = 0,
			bottom = 0,
			bg = "$zeron.lightbox.scrim",
			backdrop_blur = "$zeron.lightbox.blur",
			-- (Escape and Close do what a click here does.)
			aria_hidden = true,
			opacity = function(): number
				return math.clamp(st.lift:get(), 0, 1)
			end,
			cursor = function(): Cursor
				tick:get()
				if press and press.dragged then
					return "grabbing"
				end
				return if pannable() then "grab" else "default"
			end,
			on_press = function(e)
				stop_motion()
				press = { x = e.x, y = e.y, cx = view.cx, cy = view.cy, dragged = false, samples = { { e.x, e.y, ui.now() } } }
				if e.clicks == 2 and inside(e.x, e.y) then
					toggle_at(e.x, e.y)
				end
			end,
			on_pointer_move = function(e)
				-- (A resting pointer hears moves as the scene changes under it.)
				if last_move and last_move[1] == e.x and last_move[2] == e.y then
					return
				end
				last_move = { e.x, e.y }
				poke()
				local p = press
				if not p then
					return
				end
				local dx, dy = e.x - p.x, e.y - p.y
				if not p.dragged and math.sqrt(dx * dx + dy * dy) >= DRAG then
					p.dragged = true
				end
				if p.dragged and pannable() then
					view.cx, view.cy = p.cx + dx, p.cy + dy
					clamp_view()
					redraw()
				end
				table.insert(p.samples, { e.x, e.y, ui.now() })
				if #p.samples > 6 then
					table.remove(p.samples, 1)
				end
			end,
			on_release = function(e)
				local p = press
				press = nil
				-- A click beside the picture closes (on release, not a click
				-- handler: that would make the scrim a control, first in line
				-- for focus).
				if p and not p.dragged and not inside(p.x, p.y) and not inside(e.x, e.y) then
					st.close()
					return
				end
				if p and p.dragged and pannable() and #p.samples >= 2 then
					-- The speed over the last moves (points a millisecond).
					local a, b = p.samples[1], p.samples[#p.samples]
					local now = ui.now()
					local dt = math.max(1, b[3] - a[3])
					if now - b[3] < 60 then
						fling((b[1] - a[1]) / dt, (b[2] - a[2]) / dt)
					end
				end
			end,
			on_wheel = function(e): boolean?
				poke()
				if e.cmd or e.ctrl then
					zoom_to(view.s * math.exp(-e.dy * 0.01), e.x, e.y)
					return true
				end
				if pannable() then
					stop_motion()
					view.cx -= e.dx
					view.cy -= e.dy
					clamp_view()
					redraw()
				end
				return true
			end,
			on_pinch = function(e): boolean?
				poke()
				zoom_to(view.s * (1 + e.scale), e.x, e.y)
				return true
			end,
		},
		-- Under the picture, its lift off the scrim (light's shadow; none in
		-- dark), following the picture wherever it is drawn.
		ui.div {
			absolute = true,
			left = function(): number
				return geo().d.x
			end,
			top = function(): number
				return geo().d.y
			end,
			w = function(): number
				return math.max(1, geo().d.w)
			end,
			h = function(): number
				return math.max(1, geo().d.h)
			end,
			rounded = function(): any
				return ui.raw(geo().r, "the thumbnail's corner squaring as the picture grows from it")
			end,
			shadow = "$zeron.lightbox.shadow",
			pointer_events = "none",
			opacity = function(): number
				return math.clamp(st.alpha:get(), 0, 1)
			end,
		},
		-- The picture, in its clip.
		ui.div {
			ref = stage_ref,
			absolute = true,
			left = function(): number
				return geo().cx
			end,
			top = function(): number
				return geo().cy
			end,
			w = function(): number
				return geo().cw
			end,
			h = function(): number
				return geo().ch
			end,
			rounded = function(): any
				return ui.raw(geo().r, "the thumbnail's corner squaring as the picture grows from it")
			end,
			overflow = "hidden",
			pointer_events = "none",
			opacity = function(): number
				return math.clamp(st.alpha:get(), 0, 1)
			end,
			-- The thumbnail's own pixels, while it grows.
			ui.image {
				src = function(): string
					return if st.morphing:get() then st.snap:get() else ""
				end,
				aria_hidden = true,
				pointer_events = "none",
				-- (Its own shape, the thumbnail's box: covered, never
				-- stretched, should the two ever differ.)
				fit = "cover",
				absolute = true,
				left = function(): number
					return geo().sn.x
				end,
				top = function(): number
					return geo().sn.y
				end,
				w = function(): number
					return math.max(1, geo().sn.w)
				end,
				h = function(): number
					return math.max(1, geo().sn.h)
				end,
			},
			-- What the window showed of it as the zoom began (`hold`), under
			-- it, scaled with the zoom: its part of the window then, kept
			-- exactly where the picture is now.
			ui.show(
				function(): boolean
					local h = standin:get()
					local it = item()
					return type(h) == "table" and it ~= nil and h.item == it.src and not st.morphing:get()
				end,
				ui.div {
					absolute = true,
					overflow = "hidden",
					pointer_events = "none",
					aria_hidden = true,
					left = function(): number
						tick:get()
						local h = standin:get()
						return if h then view.cx + (h.ix - h.cx0) * view.s / h.s0 - geo().cx else 0
					end,
					top = function(): number
						tick:get()
						local h = standin:get()
						return if h then view.cy + (h.iy - h.cy0) * view.s / h.s0 - geo().cy else 0
					end,
					w = function(): number
						tick:get()
						local h = standin:get()
						return if h then math.max(1, h.iw * view.s / h.s0) else 1
					end,
					h = function(): number
						tick:get()
						local h = standin:get()
						return if h then math.max(1, h.ih * view.s / h.s0) else 1
					end,
					ui.image {
						class = "lightbox-held",
						src = function(): string
							local h = standin:get()
							return if h then h.src else ""
						end,
						aria_hidden = true,
						absolute = true,
						-- (The window's picture, at its own shape, scaled evenly.)
						fit = "cover",
						left = function(): number
							tick:get()
							local h = standin:get()
							return if h then -h.ix * view.s / h.s0 else 0
						end,
						top = function(): number
							tick:get()
							local h = standin:get()
							return if h then -h.iy * view.s / h.s0 else 0
						end,
						w = function(): number
							tick:get()
							local h = standin:get()
							return if h then math.max(1, h.w * view.s / h.s0) else 1
						end,
						h = function(): number
							tick:get()
							local h = standin:get()
							return if h then math.max(1, h.h * view.s / h.s0) else 1
						end,
					},
				}
			),
			ui.image {
				src = function(): string
					local it = item()
					return if it then it.src else ""
				end,
				on_load = loaded,
				on_error = function()
					local it = item()
					if it and not broken:peek()[it.src] then
						broken:set_key(it.src, true)
					end
				end,
				label = function(): string
					local it = item()
					return if it then it.name else ""
				end,
				-- (Decoded off the main thread: the thumbnail's own pixels,
				-- under it, stand in until it draws.)
				decode = "async",
				-- (Laid out at its own shape and scaled evenly; contained,
				-- never stretched, for the frames before a wrong size is
				-- put right.)
				fit = "contain",
				absolute = true,
				left = 0,
				top = 0,
				image_rendering = rendering,
				w = function(): number
					return math.max(1, geo().lw)
				end,
				h = function(): number
					return math.max(1, geo().lh)
				end,
				x = function(): number
					return geo().x
				end,
				y = function(): number
					return geo().y
				end,
				scale = function(): number
					return geo().k
				end,
			},
		},
		-- Its file gone: a note in the picture's place.
		ui.show(
			gone,
			ui.column {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				items = "center",
				justify = "center",
				gap = "$space.2",
				pointer_events = "none",
				class = "lightbox-gone",
				opacity = function(): number
					return math.clamp(st.alpha:get(), 0, 1)
				end,
				ui.div { color = "$text.tertiary", Icon { name = "gallery", size = 24 } },
				ui.text {
					function(): string
						local it = item()
						return if it
							then locale.s("ds.lightbox.not_shown_named", { name = it.name })
							else locale.s("ds.lightbox.not_shown")
					end,
					literal = true,
					text_size = "$font.size.md",
					color = "$text.primary",
				},
				ui.text { locale.t("ds.lightbox.gone"), text_size = "$font.size.sm", color = "$text.secondary" },
			}
		),
		-- The controls.
		ui.column {
			absolute = true,
			left = 0,
			right = 0,
			bottom = 0,
			items = "center",
			pointer_events = "none",
			-- (Up once the picture has all but landed; going, with the scrim.)
			opacity = function(): number
				local lift = st.lift:get()
				return math.clamp(if st.leaving:get() then lift else (lift - 0.6) / 0.4, 0, 1)
			end,
			ui.row(island_props),
		},
	}
end)

-- The host's body: the modal, and every motion in and out; the stage is
-- built fresh each time it opens.
local HostBody = ui.component(function(): Node
	local now = OPEN:peek()
	-- The set shown (kept while it closes), and its motion's states; open
	-- across a reload, it stands where it was, no motion.
	local shown = ui.use_state(now)
	local up = ui.use_state(now ~= nil)
	local lift = ui.use_state(if now then 1 else 0)
	local alpha = ui.use_state(if now then 1 else 0)
	local settle = ui.use_state(1)
	local t = ui.use_state(1)
	local morphing = ui.use_state(false)
	local leaving = ui.use_state(false)
	local snap = ui.use_state("")
	-- This opening has begun to move (the stage reports ready once).
	local arrived = now ~= nil
	local from: { rect: Rect?, radius: number } = { rect = nil, radius = 0 }
	local api: { rect: () -> Rect?, ready: () -> boolean, gone: () -> boolean }? = nil
	local moving: { any } = {}
	local function stop()
		for _, a in moving do
			a:cancel()
		end
		moving = {}
	end
	local function animate(target: any, params: TableParams)
		table.insert(moving, anime.animate(target, params))
	end
	-- The thumbnail of `index`, if it shows: its rect in the window, and
	-- its corner.
	local function thumb(s: Shown?): (Rect?, number)
		if not s then
			return nil, 0
		end
		local key = key_of(s.group, s.index, s.items)
		local el = if key then thumbs[key] else nil
		if not key or not el or not el.mounted or seen[key] == false then
			return nil, 0
		end
		local r = ui.window_rect(el)
		if not r or r.width <= 0 or r.height <= 0 then
			return nil, 0
		end
		return { x = r.x, y = r.y, w = r.width, h = r.height }, corners[key :: string] or 0
	end
	-- (Reduce motion, the system's or Zeron's own: ds/motion's one switch.)
	local function reduced(): boolean
		return Motion.reduced_now()
	end

	-- The stage is measured and laid out: in, from the thumbnail (or a
	-- fade, with reduced motion or no thumbnail to come from). `resume`:
	-- reopened while going, it turns back from where it is.
	local function arrive(resume: boolean?)
		stop()
		arrived = true
		local was_morphing = morphing:peek()
		leaving:set(false)
		local s = shown:peek()
		local key = if s then key_of(s.group, s.index, s.items) else nil
		local rect, radius = thumb(s)
		local snappy = Motion.SNAPPY
		if resume and was_morphing then
			-- (Back the way it came, at the pace of what is left.)
			animate(t, {
				value = 1,
				duration = ms(snappy.open * math.max(0.4, 1 - t:peek())),
				ease = snappy.ease,
				onComplete = function()
					morphing:set(false)
				end,
			})
			animate(lift, { value = 1, duration = ms(snappy.open * 0.8), ease = snappy.ease })
			return
		end
		from.rect, from.radius = rect, radius
		if rect and not reduced() and not resume and not (api and api.gone()) then
			-- The thumbnail as it showed, before it is lifted out.
			local el = if key then thumbs[key] else nil
			local shot = if el and el.mounted then el:snapshot(math.ceil(rect.w * 2)) else nil
			snap:set(shot or "")
			LIFTED:set(key)
			settle:set(1)
			morphing:set(true)
			t:set(0)
			alpha:set(1)
			animate(t, {
				value = 1,
				duration = ms(snappy.open),
				ease = snappy.ease,
				onComplete = function()
					morphing:set(false)
				end,
			})
			-- (The scrim with the picture's first, fastest part.)
			animate(lift, { value = 1, duration = ms(snappy.open * 0.8), ease = snappy.ease })
		else
			morphing:set(false)
			animate(alpha, { value = 1, duration = ms(fade_ms()), ease = "out(2)" })
			if reduced() then
				settle:set(1)
			else
				animate(settle, { value = 1, duration = ms(fade_ms()), ease = "out(2)" })
			end
			animate(lift, { value = 1, duration = ms(fade_ms()), ease = "out(2)" })
			LIFTED:set(key)
		end
	end
	-- Out: back into its thumbnail where it shows, else settling and
	-- fading where it is. Closed mid-way in, it turns back from where it is.
	local function leave()
		local was_morphing = morphing:peek()
		stop()
		leaving:set(true)
		local s = shown:peek()
		local rect, radius = thumb(s)
		local snappy = Motion.SNAPPY
		local function done()
			-- (The thumbnail shows again first, so the modal, closing, can
			-- give focus back to it: a hidden node takes none.)
			ui.batch(function()
				LIFTED:set(nil)
			end)
			up:set(false)
			-- (And if the modal had nothing to give it to, here.)
			local el = if s then thumbs[key_of(s.group, s.index, s.items) or ""] else nil
			if el and el.mounted then
				el:focus()
			end
			shown:set(nil)
			morphing:set(false)
		end
		if rect and api and api.ready() and not api.gone() and not reduced() then
			from.rect, from.radius = rect, radius
			if not was_morphing then
				t:set(1)
			end
			morphing:set(true)
			animate(t, {
				value = 0,
				duration = ms(snappy.close * math.max(0.4, t:peek())),
				ease = snappy.ease,
				onComplete = done,
			})
			animate(lift, { value = 0, duration = ms(snappy.close * 0.8), ease = snappy.ease })
		else
			morphing:set(false)
			animate(alpha, { value = 0, duration = ms(fade_ms()), ease = "out(2)" })
			if not reduced() then
				animate(settle, { value = 0.96, duration = ms(fade_ms()), ease = "out(2)" })
			end
			animate(lift, { value = 0, duration = ms(fade_ms()), ease = "out(2)", onComplete = done })
		end
	end

	ui.effect(function()
		local want = OPEN:get()
		local have = shown:peek()
		if want and not up:peek() then
			-- Opening: the stage builds, measures, then `arrive`.
			stop()
			arrived = false
			lift:set(0)
			alpha:set(0)
			t:set(0)
			snap:set("")
			shown:set(want)
			up:set(true)
		elseif want and have and leaving:peek() and want.index == have.index then
			-- Opened again while going: back from where it is.
			shown:set(want)
			arrive(true)
		elseif want and have then
			-- Another of the set: the thumbnail lifted follows it, and it
			-- fades in where it is.
			shown:set(want)
			if want.group then
				LIFTED:set(key_of(want.group, want.index, want.items))
			end
			if have.index ~= want.index then
				stop()
				morphing:set(false)
				settle:set(1)
				if reduced() then
					alpha:set(1)
				else
					alpha:set(0)
					animate(alpha, { value = 1, duration = ms(fade_ms()), ease = "out(2)" })
				end
			end
		elseif not want and up:peek() and have then
			leave()
		end
	end)
	ui.on_cleanup(stop)

	local stage: Stage = {
		shown = shown,
		lift = lift,
		alpha = alpha,
		settle = settle,
		t = t,
		morphing = morphing,
		leaving = leaving,
		snap = snap,
		from = from,
		register = function(a)
			api = a
		end,
		on_ready = function()
			-- (Once an opening. A reload finds it up and in place: nothing
			-- moves.)
			if not arrived then
				arrive()
			end
		end,
		close = M.close,
	}
	-- zgx's modal (focus held in it, given back as it goes, Escape) over
	-- the whole window; its own dim made clear, as the scrim is the stage's
	-- and fades with it.
	return ui.div {
		theme = { backdrop = "#00000000" },
		ui.modal {
			open = function(): boolean
				return up:get()
			end,
			on_close = M.close,
			label = locale.t("ds.lightbox.viewer"),
			-- (Its first control is the stage itself, which takes the keys.)
			autofocus = true,
			placement = { x = "start", y = "start", inset = 0 },
			-- (The panel is the whole window: a modal panel's size takes no
			-- percentage, and keeps 32 from the window's edges unless told.)
			w = function(): number
				return window_size(true).width
			end,
			h = function(): number
				return window_size(true).height
			end,
			max_w = function(): number
				return window_size(true).width
			end,
			max_h = function(): number
				return window_size(true).height
			end,
			p = 0,
			gap = 0,
			rounded = 0,
			bg = "#00000000",
			border = 0,
			shadow = "$zeron.dialog.stage",
			build = function()
				return StageView(stage)
			end,
		},
	}
end)

-- What makes any node a thumbnail the picture grows from and goes back
-- to (a composer chip, a message's thumbnail): its element kept, whether it
-- shows, and it lifted out while its picture is up. Add these props to it;
-- `radius` is its corner in points.
function M.source(group: string, id: string | number, radius: number?): { [string]: any }
	local key = `{group}:{id}`
	corners[key] = radius or 0
	return {
		on_mount = function(el)
			thumbs[key] = el
		end,
		on_visible = function(visible: boolean, el)
			thumbs[key] = el
			-- (Lifted out it reads as hidden: its own opacity, not a scroll.)
			if visible or LIFTED:peek() ~= key then
				seen[key] = visible
			end
		end,
		opacity = function(): number
			return if LIFTED:get() == key then 0 else 1
		end,
	}
end

-- Hosts mounted, in the order they came: the first draws, and the next
-- takes over as it goes. So a host can stand in each thing that has
-- thumbnails (a transcript, a composer) and a window still has one.
local hosts: { number } = {}
local serial = 0
local HOSTS = ui.state("hosts", 0)

-- The lightbox's host: put one where thumbnails are (more are harmless).
M.Host = ui.component(function(): Node
	serial += 1
	local me = serial
	table.insert(hosts, me)
	ui.on_cleanup(function()
		local at = table.find(hosts, me)
		if at then
			table.remove(hosts, at)
			-- (The next one takes over.)
			HOSTS:set(HOSTS:peek() + 1)
		end
	end)
	return ui.show(function(): boolean
		HOSTS:get()
		return hosts[1] == me
	end, HostBody())
end)

-- A thumbnail that opens the lightbox: a button round `child` (Enter or a
-- click opens it, and focus comes back to it), lifted out of its place
-- while its picture is up. `group` and `index` name it among its set's;
-- `radius` is its corner (in points), which the picture squares from.
export type ThumbProps = Merge<ContainerProps, {
	group: string,
	index: number,
	items: { Item } | () -> { Item },
	radius: number?,
	[number]: Child,
}>
function M.Thumb(props: ThumbProps): Node
	local list = props.items
	local function items(): { Item }
		return if type(list) == "function" then list() else list
	end
	local p: { [any]: any } = table.clone(props)
	p.group, p.index, p.items, p.radius = nil, nil, nil, nil
	local first = items()[props.index]
	p.label = p.label or (if first then locale.t("ds.lightbox.open_named", { name = first.name }) else locale.t("ds.lightbox.open"))
	p.p = p.p or 0
	p.bg = "#00000000"
	p.hover = p.hover or { bg = "#00000000" }
	p.active = p.active or { bg = "#00000000" }
	p.focus = p.focus or control.ring
	p.cursor = p.cursor or "zoom_in"
	for k, v in M.source(props.group, if first and first.key then first.key else props.index, props.radius) do
		p[k] = v
	end
	p.on_click = function()
		-- (Focus here first, so closing gives it back here.)
		local el = thumbs[`{props.group}:{if first and first.key then first.key else props.index}`]
		if el and el.mounted then
			el:focus()
		end
		M.open(items(), props.index, props.group)
	end
	return ui.button(p :: any)
end

return M

-- A new chat's wallpaper, as Zeron's: whatever stands in it (a picture of
-- your own, or what a plugin draws there) under one of Zeron's effects,
-- drawn over it as one picture (dither, ASCII, halftone, scanlines), faded
-- from the top, full, to nothing at its foot. The main window's new chat
-- draws it in its page's card; the gallery's App shell shows it (and every
-- effect).
--
--   Wallpaper {
--   	effect = state,              "none" | "dither" | "ascii" | "halftone" | "scanlines"
--   	shown = fn,                  whether it shows (a new chat's; a session has none)
--   	key = fn,                    what stands in it now ("" nothing), and
--   	content = function(key) … end   builds that, filling the art's box
--   	picture = fn,                the picture standing there (its grid, for the effects)
--   	width = state, beside = fn,  where the window is (below)
--   	provide = { … },             what its content reads (`ui.use_context`)
--   }
--
-- The art holds still in the window as its box moves under it: it's laid
-- `width` wide (the window's) from the box's right edge, moved right by
-- `beside` (what stands between the box and the window's right, a panel),
-- and the box shows what of it falls inside. A sidebar or a panel sliding
-- only moves the box's edges: the art, its effect and the effect's grid
-- (the picture's own pixels) stay where they are. A window resized lays
-- the art out again, covering it (zgx's `fit`), with no script when
-- `width` is a state.
-- Changing what stands in it, or the effect, crossfades (an effect is its
-- layer's shader, given as it is made); `intensity`, `density` and `faded`
-- change in place.
--
-- `Picture { src, w, h, x, y, zoom }` is a picture laid in it as Zeron lays
-- its new-thread background: covering the box, `zoom` times that (1 to 4),
-- the overflow cut at `x`, `y` (0 to 1: from the left or top edge, 0.5
-- centred). `w`, `h` are its size in pixels.
--
-- `follow(box, px, py)` is the props a page puts on its box so what
-- stands in it can follow the pointer (`px`, `py`, 0 to 1, 0.5 at rest).

local effects = require("@self/effects")

local M = {}

export type Props = {
	effect: (State<string> | () -> string)?,
	shown: (() -> boolean)?,
	-- What stands in it, by a key of the caller's ("" nothing): it's built
	-- again (crossfading) as the key changes.
	key: () -> string,
	content: (key: string) -> Node,
	-- The picture standing there now, if one does (read as what stands
	-- there is built): the effects work on its grid of pixels. A plugin's
	-- art has none: a pixel there is half a point.
	picture: (() -> Picture?)?,
	-- What its content reads, by name (on the box that holds it: a node
	-- that replaces what's in it sees them).
	provide: { [string]: any }?,
	-- How strongly an effect draws ("subtle" | "medium" | "strong"), and
	-- (ASCII) how many cells hold a character ("sparse" | "medium" | "dense").
	intensity: (State<string> | () -> string)?,
	density: (State<string> | () -> string)?,
	-- Faded from the top across its height (by default), or full.
	faded: (State<boolean> | () -> boolean)?,
	-- How wide the art is laid, from its box's right edge (the window's
	-- width, so it holds still as a sidebar comes and goes): a state is
	-- followed by zgx alone. Its box's, by default.
	width: (number | State<number> | () -> number)?,
	-- How far right of its box's right edge the art's right edge stands (a
	-- panel open beside the box), so it holds still as that comes and goes.
	beside: (State<number> | () -> number)?,
}

export type Picture = { src: string, w: number, h: number, x: number?, y: number?, zoom: number? }

-- The effects, in the order Settings lists them (Zeron's).
M.EFFECTS = { "none", "dither", "ascii", "halftone", "scanlines" }

-- The zoom a picture takes, at least and at most (Zeron's).
M.MIN_ZOOM, M.MAX_ZOOM = 1, 4

-- How strongly an effect draws over the picture: "medium" is Zeron's
-- whole effect; "subtle" lets some of the picture through, "strong" pushes
-- past it.
M.INTENSITY = { subtle = 0.6, medium = 1, strong = 1.3 } :: { [string]: number }
-- What each density is (ASCII): how much more of the darkest cells (the
-- lightest, in the light) than "medium" leaves without a character, or
-- (below 0) less.
M.DENSITY = { sparse = 0.25, medium = 0, dense = -0.2 } :: { [string]: number }

-- A prop's value now: a function's answer, a state's (followed), or the
-- value itself.
local function read<T>(v: any, fallback: T): T
	if v == nil then
		return fallback
	elseif type(v) == "function" then
		-- (Refining `any` by `type` leaves the solver an uncallable union.)
		local f: () -> T = v
		return f()
	elseif type(v) == "userdata" or (type(v) == "table" and getmetatable(v) ~= nil) then
		return v:get()
	end
	return v
end

-- Whether the window is light (an effect's paper follows it).
local function light(): boolean
	local kind = ui.theme.kind()
	return kind == "light" or kind == "high-contrast-light"
end

-- Where the pointer is over `box` (its ref): the props a page puts on its
-- box so what stands in its wallpaper can follow the pointer, settling
-- back as it leaves. `while_shown`: only while the wallpaper shows (a page
-- without one writes nothing as the pointer moves over it).
function M.follow(box: any, px: State<number>, py: State<number>, while_shown: (() -> boolean)?): ContainerProps
	return {
		on_pointer_move = function(e)
			if while_shown and not while_shown() then
				return
			end
			local w = box.width or 1
			local h = box.height or 1
			px:set(math.clamp((e.local_x or 0) / w, 0, 1))
			py:set(math.clamp((e.local_y or 0) / h, 0, 1))
		end,
		on_hover = function(inside: boolean)
			if not inside then
				px:set(0.5)
				py:set(0.5)
			end
		end,
	}
end

-- Where a `w` by `h` picture stands in a `bw` by `bh` box: covering it,
-- `zoom` times that, its overflow cut at `x`, `y`.
function M.cover(p: Picture, bw: number, bh: number): { left: number, top: number, w: number, h: number }
	local zoom = math.clamp(p.zoom or 1, M.MIN_ZOOM, M.MAX_ZOOM)
	local scale = math.max(bw / math.max(1, p.w), bh / math.max(1, p.h)) * zoom
	local w, h = p.w * scale, p.h * scale
	return { left = (bw - w) * math.clamp(p.x or 0.5, 0, 1), top = (bh - h) * math.clamp(p.y or 0.5, 0, 1), w = w, h = h }
end

-- A picture filling its box as `cover` lays it (a file that has gone
-- draws nothing): laid out by zgx alone (covering, its overflow cut at its
-- point, zoomed about that point: the same as `cover`), so its box
-- resizing runs no script.
function M.Picture(p: Picture): Node
	local x, y = math.clamp(p.x or 0.5, 0, 1), math.clamp(p.y or 0.5, 0, 1)
	return ui.div {
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		overflow = "hidden",
		ui.image {
			src = p.src or "",
			w_full = true,
			h_full = true,
			fit = "cover",
			object_position = { x, y },
			origin = { x, y },
			scale = math.clamp(p.zoom or 1, M.MIN_ZOOM, M.MAX_ZOOM),
			aria_hidden = true,
		},
	}
end

-- How long art takes to fade in over what it replaces.
local FADE = 320

function M.Wallpaper(props: Props): Node
	local function effect(): string
		return read(props.effect, "none")
	end
	local function shown(): boolean
		return props.shown == nil or props.shown()
	end
	-- What stands in it ("" nothing). A picture stays while the page is
	-- away (hidden): it shows again at once, decoded and drawn, not faded
	-- in anew each time a new chat opens. A plugin's art goes (it may move).
	local function what(): string
		if shown() or (props.picture and props.picture()) then
			return props.key()
		end
		return ""
	end
	-- What stands in it and the effect it's drawn through ("" nothing).
	local function key(): string
		local w = what()
		if w == "" then
			return ""
		end
		-- (The scheme isn't in it: the paper follows it in place, below.)
		return `{effect()}|{w}`
	end
	local function strength(): number
		return M.INTENSITY[read(props.intensity, "medium")] or 1
	end
	local function threshold(): number
		return M.DENSITY[read(props.density, "medium")] or 0
	end
	local function faded(): boolean
		return read(props.faded, true)
	end
	-- What stands in it and its effect, made again as either changes (the
	-- shader is fixed as it's made), crossfading: the new fades in over the
	-- old as the old fades out. What stands there is built straight in the
	-- layer's box (no presence of its own between: a plugin's `ui.change`
	-- doesn't reach a place inside a nested presence).
	-- (Hidden, not gone, while the page is away.)
	return ui.div {
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		pointer_events = "none",
		opacity = function(): number
			return if shown() then 1 else 0
		end,
		ui.presence(key, function(k: string): Node
			if k == "" then
				return ui.div {}
			end
			local how, w = string.match(k, "^([^|]*)|(.*)$")
			how = how or "none"
			local picture = if props.picture then props.picture() else nil
			-- The art's box: the window's width from the box's right edge, moved
			-- right by what stands beside the box; the effect drawn over it (on
			-- the picture's grid: the box is what the picture covers).
			return ui.div {
				-- (Named, for tests.)
				id = "wallpaper-layer",
				absolute = true,
				right = 0,
				top = 0,
				bottom = 0,
				w = props.width or "100%",
				x = props.beside,
				overflow = "hidden",
				provide = props.provide,
				-- (Away, its layer goes: zgx frees the window-sized texture it's
				-- composited from, and draws it again in one frame on return.)
				layer_on = shown,
				layer_shader = effects.shader(how),
				-- What changes in place (its strength, density, fade, the
				-- scheme's paper): followed, the layer composited again, never
				-- built again.
				layer_params = function(): { any }
					return effects.params {
						strength = strength(),
						threshold = threshold(),
						light = light(),
						faded = faded(),
						picture = picture,
					}
				end,
				props.content(w or ""),
			}
		end, {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			pointer_events = "none",
			-- The new art fades in as itself (the box the effect already draws
			-- through, not a box of its own inside it) over the old leaving.
			-- The old holds on as the new comes (an ease in against the new's
			-- ease out), so the two together never let the card show between:
			-- both easing out dimmed the art by a fifth mid-way.
			enter = { opacity = { 0, 1 }, duration = FADE, ease = "out(3)" },
			exit = { opacity = 0, duration = FADE, ease = "in(3)" },
		}),
	}
end

return M

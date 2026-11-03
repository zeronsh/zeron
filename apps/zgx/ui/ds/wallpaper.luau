-- A new chat's wallpaper, as Zeron's: whatever stands in it (a picture of
-- your own, or what a plugin draws there) under one of Zeron's effects,
-- drawn over it as one picture (dither, ASCII, halftone, scanlines), faded
-- from the top, full, to nothing at its foot. The main window's new chat
-- draws it; the gallery's App shell shows it (and every effect).
--
--   Wallpaper {
--   	effect = state,              "none" | "dither" | "ascii" | "halftone" | "scanlines"
--   	shown = fn,                  whether it shows (a new chat's; a session has none)
--   	key = fn,                    what stands in it now ("" nothing), and
--   	content = function(key) … end   builds that, filling the box
--   	provide = { … },             what its content reads (`ui.use_context`)
--   }
--
-- It fills its box, holding still as its box narrows (it's laid from the
-- box's right edge, `width` wide). Changing what stands in it, or the
-- effect, crossfades (an effect is its layer's shader, given as it is
-- made); `intensity` and `density` change in place.
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
	-- What its content reads, by name (on the box that holds it: a node
	-- that replaces what's in it sees them).
	provide: { [string]: any }?,
	-- How strongly an effect draws ("subtle" | "medium" | "strong"), and
	-- (ASCII) how many cells hold a character ("sparse" | "medium" | "dense").
	intensity: (State<string> | () -> string)?,
	density: (State<string> | () -> string)?,
	-- Faded from the top across its height (by default), or full.
	faded: (State<boolean> | () -> boolean)?,
	-- How wide it is laid, from its box's right edge (the window's width,
	-- so it holds still as a sidebar comes and goes).
	width: (number | () -> number)?,
}

export type Picture = { src: string, w: number, h: number, x: number?, y: number?, zoom: number? }

-- The effects, in the order Settings lists them (Zeron's).
M.EFFECTS = { "none", "dither", "ascii", "halftone", "scanlines" }

-- The zoom a picture takes, at least and at most (Zeron's).
M.MIN_ZOOM, M.MAX_ZOOM = 1, 4

-- What each strength and density is, on the effect's shader.
M.INTENSITY = { subtle = 0.45, medium = 0.75, strong = 1 } :: { [string]: number }
M.DENSITY = { sparse = 0.38, medium = 0.22, dense = 0.08 } :: { [string]: number }

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
-- draws nothing).
function M.Picture(p: Picture): Node
	local box = ui.ref()
	local function at(): { left: number, top: number, w: number, h: number }
		return M.cover(p, box.width or 0, box.height or 0)
	end
	return ui.div {
		ref = box,
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		overflow = "hidden",
		ui.image {
			src = p.src or "",
			absolute = true,
			left = function(): number
				return at().left
			end,
			top = function(): number
				return at().top
			end,
			w = function(): number
				return at().w
			end,
			h = function(): number
				return at().h
			end,
			aria_hidden = true,
		},
	}
end

-- How long art takes to fade in over what it replaces.
local FADE = 320

function M.Wallpaper(props: Props): Node
	local box = ui.ref()
	local function effect(): string
		return read(props.effect, "none")
	end
	-- What stands in it ("" nothing).
	local function what(): string
		local shown = props.shown == nil or props.shown()
		return if shown then props.key() else ""
	end
	-- What stands in it and the effect it's drawn through ("" nothing).
	local function key(): string
		local w = what()
		if w == "" then
			return ""
		end
		local how = effect()
		-- (Halftone's and the lines' paper follow the scheme.)
		local paper = if how == "halftone" or how == "scanlines" then (if light() then ":light" else ":dark") else ""
		return `{how}{paper}|{w}`
	end
	local function strength(): number
		return M.INTENSITY[read(props.intensity, "medium")] or 0.75
	end
	local function threshold(): number
		return M.DENSITY[read(props.density, "medium")] or 0.22
	end
	-- What stands in it and its effect, made again as either changes (the
	-- shader is fixed as it's made), crossfading: the new fades in over the
	-- old as the old fades out. What stands there is built straight in the
	-- layer's box (no presence of its own between: a plugin's `ui.change`
	-- doesn't reach a place inside a nested presence). The new art fades in
	-- inside its layer over the old layer leaving.
	return ui.presence(key, function(k: string): Node
		if k == "" then
			return ui.div {}
		end
		local how, w = string.match(k, "^([^|]*)|(.*)$")
		how = string.match(how or "", "^[^:]*") or "none"
		local el = ui.ref()
		local scene: { [any]: any } = {
			ref = el,
			w_full = true,
			h_full = true,
			relative = true,
			overflow = "hidden",
			provide = props.provide,
			fade = function()
				return { 0, if read(props.faded, true) then (box.height or 0) else 0 }
			end,
			-- (Named, for tests.)
			id = "wallpaper-layer",
			-- What stands there, laid at the window's width from the right
			-- edge so it holds still as a sidebar comes and goes; the box
			-- (and its layer) only as wide as what shows of it.
			ui.div {
				absolute = true,
				right = 0,
				top = 0,
				bottom = 0,
				w = props.width or 820,
				props.content(w or ""),
			},
		}
		-- (params[0]: the cell, the strength, the threshold; set in place.
		-- Halftone and the lines draw at a share of it: theirs is the
		-- picture's whole surface, not its bright cells.)
		local share = if how == "halftone" then 0.8 elseif how == "scanlines" then 0.64 else 1
		if how ~= "none" then
			ui.effect(function()
				local strong, dense = strength() * share, threshold()
				local node = el:peek()
				if node then
					anime.utils.set(node, { layerParam1 = strong, layerParam2 = dense })
				end
			end)
		end
		if how == "dither" then
			scene.layer_shader = effects.dither
			scene.layer_params = effects.dither_params(2, strength())
		elseif how == "ascii" then
			scene.layer_shader = effects.ascii
			scene.layer_params = effects.ascii_params(1.5, strength(), threshold())
		elseif how == "halftone" then
			scene.layer_shader = effects.halftone
			scene.layer_params = effects.halftone_params(4, strength() * share, light())
		elseif how == "scanlines" then
			scene.layer_shader = effects.scanlines
			scene.layer_params = effects.scanlines_params(3, strength() * share, light())
		end
		return ui.div(scene)
	end, {
		ref = box,
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		pointer_events = "none",
		-- The new art fades in as itself (the box the effect already draws
		-- through, not a box of its own inside it) over the old leaving.
		enter = { opacity = { 0, 1 }, duration = FADE, ease = "out(3)" },
		exit = { opacity = 0, duration = FADE, ease = "out(3)" },
	})
end

return M

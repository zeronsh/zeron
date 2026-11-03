-- A scrollbar as Zeron draws one: a thin thumb floating over a scroll
-- view's trailing edge, there while the view scrolls and for a moment
-- after, or while the pointer is on it, and gone otherwise, so a list
-- shows its rows and nothing else. It takes the pointer as a scrollbar
-- does: dragged, the view follows; pressed beside the thumb, the thumb
-- comes under the pointer and follows from there.
--
-- The engine's own bar (`scrollbar = true`) is a wider one at the view's
-- very edge, shown as the system shows scrollbars; this one is the app's
-- (popover.rs's floating rail): a 3-point thumb, 5 under the pointer, 2 in
-- from the edge, its ends 4 in from the view's so it clears a card's
-- corners, lingering 1.4 s after the last scroll and fading over 0.26 s.
-- Hidden, it takes no pointer: what is under it does.
--
-- It lays over the scroll view: a `relative` box holds the view and, after
-- it, `Rail { offset, extent, view }`: the view's offset (the state the
-- view scrolls by), how long its content is, and how long the view is.
--
-- `ui.div { relative = true, ui.list { offset = at, … }, Rail { offset = at, extent = total, view = seen } }`
--
-- It is the scroll view's sibling, not its child, on purpose: a view's edge
-- `fade` masks everything inside it, the engine's own bar included, so a
-- faded view's bar would fade with its rows. Over the view, the rail is
-- whole along its track. `track`: how long that is, where the rail stops
-- short of the view's end (a transcript's, above the composer laid over
-- its foot); the view's length by default.

export type Length = (() -> number) | State<number>

export type Props = {
	-- The scroll view's offset: read for the thumb, written as it is dragged.
	offset: State<number>,
	-- The content's length, and the view's.
	extent: Length,
	view: Length,
	track: Length?,
	-- An id for the strip, for tools that look for it.
	id: string?,
}

-- The rail's geometry: the strip that takes the pointer, the thumb at rest
-- and under the pointer, its room from the edge, the track's ends in from
-- the view's, the shortest thumb; how long it stays and how long it fades.
local HIT = 10
local THUMB, THUMB_ACTIVE = 3, 5
local SIDE = 2
local END = 4
local MIN = 24
local LINGER, FADE = 1400, 260
-- How long a view is taken to place itself once it mounts, its offset's
-- changes meanwhile no scroll.
local SETTLE = 600

local function read(v: Length): number
	return if type(v) == "function" then v() else v:get()
end

return ui.component(function(props: Props): Node
	local offset = props.offset
	local hovered = ui.use_state(false)
	local dragging = ui.use_state(false)
	-- The view scrolled a moment ago.
	local recent = ui.use_state(false)
	local grab = 0

	-- Where the thumb stands: its top and length, how far it can travel,
	-- and how far the view can scroll; nothing when the content fits.
	-- (What the view and its content make, apart: a scroll moves only the
	-- thumb's top, so only that is worked out again as it scrolls.)
	type Span = { fits: boolean, length: number, travel: number, limit: number }
	local span = ui.computed(function(): Span
		local view, extent = read(props.view), read(props.extent)
		local track = (if props.track then read(props.track) else view) - 2 * END
		local limit = extent - view
		if limit <= 0 or track <= 0 then
			return { fits = true, length = 0, travel = 0, limit = 0 }
		end
		local length = math.max(MIN, math.min(track, track * view / extent))
		return { fits = false, length = length, travel = math.max(0, track - length), limit = limit }
	end)
	local function geometry(): (number?, number, number, number)
		local sp = span:get()
		if sp.fits then
			return nil, 0, 0, 0
		end
		local at = math.clamp(offset:get(), 0, sp.limit)
		return END + sp.travel * at / sp.limit, sp.length, sp.travel, sp.limit
	end

	-- A change of offset is a scroll: the rail shows, and goes once the view
	-- has rested. Not while the view places itself as it mounts (a kept
	-- place put back as its rows are measured, as a chat opens or the
	-- scripts reload): that is no one scrolling, and a reload once lit the
	-- rail and drew some 60 frames for it.
	local settled = false
	local settling: (() -> ())? = nil
	settling = ui.after(SETTLE, function()
		settling = nil
		settled = true
	end)
	local cancel: (() -> ())? = nil
	ui.effect(function()
		offset:get()
		if not settled then
			return
		end
		recent:set(true)
		if cancel then
			cancel()
		end
		cancel = ui.after(LINGER, function()
			cancel = nil
			recent:set(false)
		end)
	end)
	ui.on_cleanup(function()
		if cancel then
			cancel()
		end
		if settling then
			settling()
		end
	end)

	local function active(): boolean
		return hovered:get() or dragging:get()
	end
	local function shown(): boolean
		return not span:get().fits and (recent:get() or active())
	end

	-- Where the pointer is along the track, from its top.
	local function along(e: { local_y: number }): number
		return e.local_y - END
	end
	local function follow(e: { local_y: number })
		local top, _, travel, limit = geometry()
		if top == nil or travel <= 0 then
			return
		end
		local to = math.clamp(along(e) - grab, 0, travel)
		offset:set(to / travel * limit)
	end

	return ui.div {
		id = props.id,
		absolute = true,
		top = 0,
		h = if props.track
			then function(): number
				return read(props.track :: Length)
			end
			else nil,
		bottom = if props.track then nil else 0,
		right = 0,
		w = HIT,
		aria_hidden = true,
		opacity = ui.tween(
			ui.computed(function(): number
				return if shown() then 1 else 0
			end),
			{ duration = FADE, ease = "outQuad" }
		),
		pointer_events = function(): any
			return if shown() then "auto" else "none"
		end,
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		on_press = function(e)
			local top, length = geometry()
			if top == nil then
				return
			end
			-- On the thumb, it keeps its place under the pointer; beside it,
			-- the thumb's middle comes to the pointer.
			local at = along(e)
			local thumb_top = top - END
			grab = if at >= thumb_top and at <= thumb_top + length then at - thumb_top else length / 2
			dragging:set(true)
			follow(e)
		end,
		on_pointer_move = function(e)
			if dragging:peek() then
				follow(e)
			end
		end,
		on_release = function()
			dragging:set(false)
		end,
		ui.div {
			absolute = true,
			right = SIDE,
			top = function(): number
				local top = geometry()
				return top or END
			end,
			w = function(): number
				return if active() then THUMB_ACTIVE else THUMB
			end,
			h = function(): number
				return span:get().length
			end,
			rounded = "$radius.full",
			bg = function(): string
				return if active() then "$scrollbar.thumb.active" else "$scrollbar.thumb"
			end,
			pointer_events = "none",
			aria_hidden = true,
		},
	}
end)

-- Where a page is scrolled, kept: the place the person scrolled to, held in
-- a state of the page's own (a named one, so the page made again, by a
-- trip to another page and back or the scripts reloaded, opens where it
-- was), and never lost to a layout that is short for a while.
--
-- A scroll view keeps its offset clamped to what its content holds, and
-- writes the clamped value back over the one it was given. So a page whose
-- content is short for a moment (made again before its rows have come, a
-- widget swapped for another, a list built again) would land at its top,
-- or as far down as the short content went, and stay there once the
-- content was back. Here only the person moves the place: the wheel or a
-- trackpad, a press (and the focus it moves), the keys that scroll; or the
-- view going further down than the place, which a clamp never does. A move
-- the view makes alone, to fit, is not the place, and the place comes back
-- as the content grows to hold it again.
--
--   local place = ui.state("control_scroll", 0)   -- in the page's module
--   local kept = KeptScroll.use(place)             -- in its component
--   ui.scroll { offset = kept.offset,
--     on_wheel = kept.on_wheel, on_press = kept.on_press, on_key = kept.on_key,
--     ui.column { on_resize = kept.on_resize, … } }
--
-- `on_resize` goes on the scroll's one child (what it holds); a page that
-- wants its size too calls it from its own.
--
--   KeptScroll.scroll_by(place, dy)   -- a wheel something in the page took
--
-- moves the page as the person's wheel would (a field that keeps the wheel
-- only while it's being worked in hands it to the page this way).

local M = {}

-- Each place's `touch`, while its page is up: what `scroll_by` marks the
-- move the person's with.
local touches: { [any]: () -> () } = setmetatable({}, { __mode = "k" }) :: any

-- How long after the person's last input, or the last move that was
-- theirs, the view's moves are still theirs: a wheel's smooth glide and a
-- trackpad's momentum move it a frame at a time, each move keeping it so.
local HELD_MS = 500

-- The keys that scroll a page, or move focus through it.
local SCROLL_KEYS: { [string]: boolean } = {
	PageUp = true,
	PageDown = true,
	Home = true,
	End = true,
	ArrowUp = true,
	ArrowDown = true,
	Space = true,
	[" "] = true,
	Tab = true,
}

export type Kept = {
	-- The scroll's `offset`.
	offset: any,
	-- The scroll's own: the person's input.
	on_wheel: (e: any) -> boolean?,
	on_press: (e: any) -> (),
	on_key: (e: any) -> boolean?,
	-- What the scroll holds, laid out: its child's `on_resize`.
	on_resize: (size: any, element: any) -> (),
}

function M.use(place: any): Kept
	local shown = ui.use_state(place:peek() :: number)
	-- When the person last scrolled it.
	local touched = -math.huge
	local function theirs(): boolean
		return ui.now() - touched < HELD_MS
	end
	-- The view moved: the place, when it's the person's move (or further
	-- down than the place: never a clamp).
	ui.effect(function()
		local y = shown:get()
		local want = place:peek()
		if math.abs(y - want) > 0.5 and (y > want or theirs()) then
			if theirs() then
				touched = ui.now()
			end
			place:set(y)
		end
	end)
	-- The place set from elsewhere: the view goes there (as far as it can).
	ui.effect(function()
		local want = place:get()
		if math.abs(want - shown:peek()) > 0.5 then
			shown:set(want)
		end
	end)
	local function touch()
		touched = ui.now()
	end
	touches[place] = touch
	return {
		offset = shown,
		on_wheel = function(_e: any): boolean?
			touch()
			return nil
		end,
		on_press = function(_e: any)
			touch()
		end,
		on_key = function(e: any): boolean?
			if SCROLL_KEYS[e.key] and (e.key == "Tab" or not e.editing) then
				touch()
			end
			return nil
		end,
		on_resize = function(_size: any, _element: any)
			-- Grown, short of the place it was moved off to fit: back
			-- toward it (as far as the content now goes).
			local want = shown:peek()
			if want < place:peek() - 0.5 and not theirs() then
				shown:set(place:peek())
			end
		end,
	}
end

-- Move the page `dy` points (down positive), as the person's wheel: the
-- view goes as far as it can and the place follows it.
function M.scroll_by(place: any, dy: number)
	local touch = touches[place]
	if touch then
		touch()
	end
	place:set(math.max(0, place:peek() + dy))
end

return M

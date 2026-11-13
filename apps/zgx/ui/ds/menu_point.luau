-- Where a context menu hangs (ds/context_menu): a 1-point box at a point of
-- the window, and what it may show.
--
-- The point is kept on whole window points. The compositor places a menu
-- against that box in whole points: one between them (a chip's corner at
-- 288.5, in a centred column of an odd width) rounded to a box of no width
-- under the menu beside it, and Wayland compositors close the app over a
-- box like that (Hyprland's "Invalid box").

local M = {}

-- `n` on the nearest whole point (halves up).
function M.whole(n: number): number
	return math.floor(n + 0.5)
end

-- The point `x`, `y` (the window's), on whole points, from the top left of
-- a box at `ox`, `oy`: where its 1-point box goes in that box.
function M.at(x: number, y: number, ox: number, oy: number): (number, number)
	return M.whole(x) - ox, M.whole(y) - oy
end

-- Whether `entries` hold anything to choose: a menu of only headings and
-- rules (or none) opens no panel.
function M.has_items(entries: { any }?): boolean
	for _, e in entries or {} do
		if e.label ~= nil and not e.separator and e.heading == nil then
			return true
		end
	end
	return false
end

return M

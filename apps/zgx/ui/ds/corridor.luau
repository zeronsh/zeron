-- The way to an open submenu: while the pointer crosses other rows of its
-- menu heading for it, those rows wait before taking over (a context
-- menu's submenus, ds/context_menu; a composer's Work in rows,
-- ds/composer).
--
--   corridor.toward(o, p, b, left)   whether the pointer at `p`, having last
--                                    rested on the trigger at `o`, is still
--                                    heading for the submenu `b`
--   corridor.GRACE, SLACK, NUDGE     how long a row waits (renewed while the
--                                    pointer keeps on toward it), how far past
--                                    the submenu's edges the corridor reaches,
--                                    how far a move must go to count
--
-- Pure: no state.

local M = {}

M.GRACE, M.SLACK, M.NUDGE = 300, 8, 2

-- Inside the triangle from `o` to the submenu's near edge (on the left of
-- `o`, `left`), its ends `SLACK` past the submenu's top and foot.
function M.toward(o: { x: number, y: number }, p: { x: number, y: number }, b: Element, left: boolean): boolean
	local bx, by, bw, bh = b.x, b.y, b.width, b.height
	if not (bx and by and bw and bh) then
		return false
	end
	local slack = M.SLACK
	local edge = if left then bx + bw else bx
	local dir = if left then -1 else 1
	local distance = (edge - o.x) * dir
	local advance = (p.x - o.x) * dir
	if distance <= 0 or advance <= 0 or advance > distance + slack then
		return false
	end
	local f = math.min(advance / distance, 1)
	local top = o.y + (by - slack - o.y) * f
	local bottom = o.y + (by + bh + slack - o.y) * f
	return p.y >= top and p.y <= bottom
end

return M

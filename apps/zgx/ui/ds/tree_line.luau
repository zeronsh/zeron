-- A tree's branch: a trunk down a row and an arm curving off it to the row's
-- mark, drawn in one even tone. The lines are the text's ink, faint, so two
-- strokes laid over each other go darker where they cross: a trunk and an
-- arm stroked apart double along the curve, where it leaves the trunk. So
-- the two are one outline, filled once (as Zeron's `activity_rail` unions
-- them), and the tone holds through the joint.
--
-- `c:fill(tree_line.branch { x = 12, mid = 13, tip = 24, h = 28, bend = 6, on = true }, "$zeron.tree")`
-- `on`: the trunk goes on below the arm; `h` is where it stops.

local M = {}

local HALF = 0.5

export type Branch = { x: number, mid: number, tip: number, h: number, bend: number, on: boolean }

function M.branch(b: Branch): any
	local x, mid, tip, h, bend = b.x, b.mid, b.tip, b.h, b.bend
	local path = ui.path():move(x - HALF, 0)
	if b.on then
		-- The arm's foot runs as a curve from the arm into the trunk's left
		-- side; only the part of it outside the trunk is an edge. Where it
		-- crosses the trunk's right side, it stops.
		local t = 1 - math.sqrt(2 * HALF / (bend + HALF))
		local cx, cy = x + bend + (x - HALF - (x + bend)) * t, mid + HALF
		local y2 = mid + HALF + (mid - bend - (mid + HALF)) * t * t
		path = path:line(x - HALF, h):line(x + HALF, h):line(x + HALF, y2):quad(cx, cy, x + bend, mid + HALF)
	else
		path = path:line(x - HALF, mid - bend):quad(x - HALF, mid + HALF, x + bend, mid + HALF)
	end
	return path:line(tip, mid + HALF)
		:line(tip, mid - HALF)
		:line(x + bend, mid - HALF)
		:quad(x + HALF, mid - HALF, x + HALF, mid - bend)
		:line(x + HALF, 0)
		:close()
end

-- A trunk going on down a row without a branch off it (an outer group's,
-- past an inner group's rows): from its top to `h`.
function M.trunk(x: number, h: number): any
	return ui.path():move(x - HALF, 0):line(x - HALF, h):line(x + HALF, h):line(x + HALF, 0):close()
end

return M

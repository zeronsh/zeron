-- Text that doesn't fit fades out at its end, never cut to "…": the
-- design system's one way to truncate. It fades only while it overflows (a
-- label that fits ends crisp), measured as it lays out.
--
-- `FadeText { title, grow = 1, text_size = "$font.size.md" }` (the first entry:
-- a string, a message, a state or a function). Text props style it; `grow`, `shrink`,
-- `basis`, `w`, `w_full`, `min_w`, `max_w` and the margins go to its box. `fade`: the
-- fade's length (24 points); `roll = true`: a RollText (it rolls as it
-- changes; `glide = false` as RollText takes it).

local RollText = require("@self/roll_text")

-- Props that belong to the box, not the text.
local BOX = {
	grow = true,
	shrink = true,
	basis = true,
	w = true,
	w_full = true,
	min_w = true,
	max_w = true,
	mt = true,
	mb = true,
	ml = true,
	mr = true,
	self = true,
	opacity = true,
}

return ui.component(function(props: { [any]: any }): Node
	local clip, text = ui.ref(), ui.ref()
	local fade = props.fade or 24
	local box: { [any]: any } = {}
	local style: { [any]: any } = {}
	for k, v in props do
		if BOX[k] then
			box[k] = v
		elseif k ~= "fade" and k ~= "roll" and k ~= "children" and k ~= "truncate" then
			style[k] = v
		end
	end
	-- (`glide`: a RollText's, as `roll` is.)
	if not props.roll then
		style.glide = nil
	end
	-- The text at its own width, inside a box that clips it.
	style.ref = text
	style.shrink = 0
	style.wrap = nil
	local inner = if props.roll then RollText(style) else ui.text(style)
	box.ref = clip
	box.min_w = box.min_w or 0
	box.overflow = "hidden"
	box.items = "center"
	box.fade_x = function()
		local over = (text.width or 0) > (clip.width or 0) + 0.5
		return { 0, if over then fade else 0 }
	end
	box[1] = inner
	-- A row, so the text keeps its width inside (a column would stretch it to
	-- the box's, and nothing would overflow).
	return ui.row(box)
end)

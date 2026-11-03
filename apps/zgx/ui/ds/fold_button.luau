-- The small, quiet button that opens a long block in place and folds it
-- back: a thought's words in a reply's work ("Show all N lines", "Show fewer
-- lines"), a command's output under its call. On the secondary fill, in the
-- small type, medium; its ring for the keys. Where it sits is its caller's
-- (`place`: the layout props it takes, as `{ absolute = true, left = 0,
-- bottom = 0 }` under a thought's faded words, or `{ self = "start" }`).
--
--   FoldButton { label = function() … end, open = is_open, on_click = toggle, place = { … } }

local control = require("@self/control")

local M = {}

-- Its height, for a caller leaving room for it.
M.HEIGHT = 20

export type Props = {
	label: () -> string,
	open: () -> boolean,
	on_click: () -> (),
	class: string?,
	place: { [string]: any }?,
}

function M.FoldButton(props: Props): Node
	local node: { [any]: any } = {
		label = props.label,
		expanded = props.open,
		class = props.class,
		h = M.HEIGHT,
		px = "$zeron.space.tight",
		items = "center",
		rounded = "$radius.sm",
		bg = "$zeron.secondary.bg",
		color = "$text.secondary",
		hover = { bg = "$zeron.secondary.hover", color = "$text.primary" },
		active = { bg = "$zeron.secondary.active", color = "$text.primary" },
		focus = control.ring,
		cursor = "pointer",
		selectable = false,
		on_click = props.on_click,
		ui.text {
			props.label,
			text_size = "$font.size.xs",
			weight = "$font.weight.medium",
		},
	}
	for k, v in props.place or {} do
		node[k] = v
	end
	return ui.button(node)
end

return M

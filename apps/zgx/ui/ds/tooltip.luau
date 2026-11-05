-- A tooltip whose words follow a state: `tooltip = Tooltip { function() … end }`.
-- zgx draws a string tooltip in the theme's look, but a node as it is (no
-- fill, no edge), so live words are framed here: zgx's tip's size (8 by 4
-- inset, 320 at most, the extra-small text) in ds/hover_card's frame, the
-- menus' glass, as every node tooltip is.

local HoverCard = require("@self/hover_card")

-- (A message, `ui.t`, too: `tooltip = Tooltip { ui.t(…) }` follows the
-- locale, where a string tooltip is read once.)
export type Props = { [number]: string | Message | () -> string }

return function(props: Props): Node
	return HoverCard.Tip {
		px = "$space.2",
		py = "$space.1",
		max_w = 320,
		ui.text { props[1], text_size = "$font.size.xs", color = "$text.primary", wrap = true },
	}
end

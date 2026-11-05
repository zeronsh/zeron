-- What shows as the pointer rests on something (a node `tooltip`): a card,
-- or a tip of words, framed once, in the menus' own surface: the glass the
-- pickers' panels wear (ds/glass `menu`: the menus' tint over a blur of
-- what's behind, their edge, their shadow, their corner), so a hover card
-- and a menu are one material. Every node tooltip in the app draws through
-- here (tests/hover_cards.rs fails on one that doesn't).
--
-- zgx shows a node tooltip in a popup window of its own, its panel zgx's
-- clear layer hugging the card. Since zgx b75d8e74f (#1390) the popup finds
-- the card through that layer: a card with its own shadow turns the window
-- server's off (no second rim round it), and a translucent card gets
-- AppKit's material under it, cut to its corners, as a menu's panel does
-- (zgx-asks/popup-layer-shadow, adopted).
--
--   tooltip = HoverCard.Card { w = 248, p = "$space.1", … },
--   tooltip_placement = { side = "top", align = "end", offset = 8 },
--
-- `Card` and `Tip` (the small one words take: ds/tooltip) are the same
-- surface; each takes a column's props and children. `frame(props)` adds
-- it to props of the caller's own.

local glass = require("@self/glass")

local M = {}

export type Kind = "card" | "tip"

-- Adds the frame to `props` (changed in place, and returned): the menus'
-- glass, once; what it holds paints none of its own round it.
function M.frame(props: { [any]: any }, _kind: Kind?): { [any]: any }
	props.class = if props.class then `{props.class} hover-card-frame` else "hover-card-frame"
	return glass.style(props, "menu")
end

export type Props = Merge<ContainerProps, { [number]: Child }>

-- A card: a column in the frame.
function M.Card(props: Props): Node
	return ui.column(M.frame(table.clone(props), "card"))
end

-- A tip of words: a column in the frame.
function M.Tip(props: Props): Node
	return ui.column(M.frame(table.clone(props), "tip"))
end

return M

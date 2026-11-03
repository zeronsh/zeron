--!strict
-- A row of a list that stands in a card or a pane (a file in Changes, a
-- commit in History, a pull request on the board, a file in a pull
-- request): the one way such a row answers the pointer, so every list in
-- every theme does the same.
--
-- It is a Surface (ds/surface) at `depth = "flat"`, as the sidebar's rows
-- are: under the pointer the theme's `$hover` wash, pressed `$pressed`,
-- the chosen one on the flat selection (`$zeron.selected.flat.bg`). Flat
-- means the wash alone: no edge, no lift, no shadow, no highlight. And in
-- a card it washes edge to edge, with no corner of its own: the card's
-- corner clips the first row and the last, so a row under the pointer
-- never reads as a card inside the card (rows on the window's glass, the
-- sidebar's, inset and round themselves instead).
--
--   ListRow { selected = function() … end, on_click = open, label = "…", row }
--
-- Enter and Space press it, as a button's do; it is one Tab stop.

local Surface = require("@self/surface")

export type Props = {
	selected: (() -> boolean)?,
	on_click: (() -> ())?,
	on_hover: ((inside: boolean) -> ())?,
	label: any?,
	class: string?,
	id: string?,
	provide: { [string]: any }?,
	role: string?,
	h: any?,
	[number]: Node,
}

-- What it's drawn with, for what checks that every list hovers alike.
local M = {}
M.HOVER, M.PRESSED, M.SELECTED = "$hover", "$pressed", "$zeron.selected.flat.bg"

function M.ListRow(props: Props): Node
	local on_click = props.on_click
	local box: { [any]: any } = {
		id = props.id,
		class = props.class,
		provide = props.provide,
		depth = "flat",
		selected = props.selected or false,
		on_click = on_click,
		on_hover = props.on_hover,
		role = props.role or "button",
		label = props.label,
		focusable = on_click ~= nil,
		-- (Edge to edge in its card: no corner, no edge of its own.)
		rounded = 0,
		border = 0,
		w_full = true,
		h = props.h,
		shrink = 0,
		on_key = if on_click
			then function(e: any): boolean?
				if (e.key == "Enter" or e.key == " " or e.key == "Space") and not (e.cmd or e.ctrl or e.alt) then
					on_click()
					return true
				end
				return nil
			end
			else nil,
	}
	for i, child in props :: any do
		if type(i) == "number" then
			box[i] = child
		end
	end
	return Surface(box :: any)
end

return M

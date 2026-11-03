-- A section's title: the small, quiet label over a group of things (a
-- page's run of examples, a settings group, a sidebar's group, a panel's
-- part). One look everywhere: the label style in the tertiary ink.
--
-- `SectionTitle { "Appearance" }`. `trail`: something at its end (a count,
-- a small action), pushed to the far edge. Other props go to the text (or,
-- with a trail, to the row round it).

local Text = require("@self/text")

export type Props = Merge<TextProps, { trail: Node?, [number]: string | Message | () -> string }>

return function(props: Props): Node
	local p = table.clone(props) :: any
	local trail = p.trail
	p.trail = nil
	p.style = "label"
	p.tone = "tertiary"
	if not trail then
		return Text(p)
	end
	local title = p[1]
	p[1] = nil
	p.items = p.items or "center"
	p.gap = p.gap or "$space.2"
	return ui.row {
		w_full = true,
		items = p.items,
		gap = p.gap,
		Text { title, style = "label", tone = "tertiary", grow = 1 },
		trail,
	}
end

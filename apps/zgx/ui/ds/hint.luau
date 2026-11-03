-- A hint: a line of help under or beside what it is about (a field's
-- note, an empty list's next step, a shortcut's reminder). Caption type in
-- the tertiary ink, wrapping to its box.
--
-- `Hint { "Enter replaces, ⌘Enter replaces all." }`. `tone = "danger"`
-- for a problem in place of the help (a field's error). Other props go to
-- the text.

local Text = require("@self/text")

export type Props = Merge<TextProps, { tone: ("tertiary" | "secondary" | "danger")?, [number]: string | Message | () -> string }>

return function(props: Props): Node
	local p = table.clone(props) :: any
	p.style = "caption"
	p.tone = p.tone or "tertiary"
	if p.wrap == nil then
		p.wrap = true
	end
	return Text(p)
end

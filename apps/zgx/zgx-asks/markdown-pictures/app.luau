-- Repro: pictures in Markdown as ui.markdown parses them. The blocks of
-- three texts, each kind by kind ("p", "img", …), are put in states for the
-- test to read.

local function kinds(text: string): string
	local out = {}
	for _, b in ui.markdown(text) do
		local t = b.t
		if t == "p" and (b.s == nil or #b.s == 0) then
			t = "p(empty)"
		end
		table.insert(out, t)
	end
	return table.concat(out, ",")
end

-- A picture on the line under some words (an agent's "Here it is:").
ui.state("under_words", kinds("Here it is:\n![shot](a.png)"))
-- Two pictures on lines of their own, one under the other.
ui.state("two_lines", kinds("![a](a.png)\n![b](b.png)"))
-- A picture alone in its paragraph.
ui.state("alone", kinds("![a](a.png)\n\nAfter."))

return ui.column {}

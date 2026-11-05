-- Repro: a paragraph that needs a drawn thing inside its text, on its
-- baseline, wrapping with the words (inline LaTeX math, as Zeron's replies
-- show it: "where $e^{i\pi} + 1 = 0$ holds").
--
-- Today a paragraph is `ui.rich` of `ui.span`s, and the only non-text a span
-- holds is `icon = { src, size }`: a square SVG centred in the line. A
-- formula is as wide as it is (here 74 × 22, 6 of it below the baseline),
-- so neither fits.

local FORMULA = { w = 74, ascent = 16, descent = 6 }

-- What we draw for the formula (a stand-in: a box of its size).
local function formula(): Node
	return ui.canvas {
		w = FORMULA.w,
		h = FORMULA.ascent + FORMULA.descent,
		draw = function(c: any)
			c:rect(0, 0, FORMULA.w, FORMULA.ascent + FORMULA.descent, "#888888")
		end,
	}
end

-- 1. A node among the spans: rejected (a rich paragraph takes spans).
local ok, why = pcall(function()
	return ui.rich { "where ", formula(), " holds, and the words wrap around it as a word would." }
end)
ui.state("node_in_rich", if ok then "accepted" else tostring(why))

-- 2. The closest thing today: a row of text, node, text. It can't wrap: a
-- long paragraph overflows or breaks in the wrong place, and the node sits
-- on the row's centre, not the text's baseline.
return ui.column {
	w = 320,
	gap = 16,
	p = 16,
	ui.row {
		id = "row",
		items = "end",
		ui.text { "where " },
		formula(),
		ui.text { " holds, and the words wrap around it as a word would." },
	},
}

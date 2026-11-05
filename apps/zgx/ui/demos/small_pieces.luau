-- The gallery's Section title and Hint page: the small text pieces every
-- surface shares.

local ds = require("@ds")

return {
	id = "small-pieces",
	title = "Section title, hint",
	about = "The quiet label over a group of things, and a line of help under what it is about.",
	demo = function(): Node
		return ui.column {
			gap = "$space.6",
			w = 360,
			ui.column {
				gap = "$space.3",
				ds.SectionTitle { "SECTION TITLE" },
				ds.Text { "What the section holds." },
			},
			ui.column {
				gap = "$space.3",
				ds.SectionTitle { "WITH A TRAIL", trail = ds.IconButton { icon = "plus", label = "Add", size = "sm", tone = "quiet" } },
				ds.Text { "A trail sits at the far edge." },
			},
			ui.column {
				gap = "$space.1",
				ds.Text { "A field's label" },
				ds.Hint { "A hint: help under what it is about, wrapping to its box when it runs longer than a line." },
				ds.Hint { "A problem in place of the help.", tone = "danger" },
			},
		}
	end,
}

-- What the primitives' gallery pages share: a titled block of examples and a
-- row of them, as the gallery's own pages lay theirs out.

local ds = require("@ds")

local M = {}

-- A row of examples, wrapping.
function M.row(...: Child): Node
	return ui.row { gap = "$space.3", items = "center", flex_wrap = true, ... }
end

-- A titled run of examples on a page.
function M.block(title: string, ...: Child): Node
	return ui.column {
		gap = "$space.3",
		ds.Text { title, style = "label", tone = "tertiary" },
		...,
	}
end

-- A page's column of blocks.
function M.page(...: Child): Node
	return ui.column { gap = "$space.6", ... }
end

-- A column of examples at a form's width.
function M.form(...: Child): Node
	return ui.column { gap = "$space.4", w = 320, ... }
end

return M

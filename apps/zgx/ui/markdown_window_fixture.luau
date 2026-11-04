-- A reply's windowed words, for tests/markdown_window: a run judged away from
-- the view in the moment between its row's `on_resize` and its own, as a
-- frame that writes the offset from a row's `on_resize` can do.

local Markdown = require("@ds/markdown")

local PARAGRAPH = "The layout pass runs first and measures every row it shows, then paint fills the scene, then present hands the frame to the display; a width change asks layout again for every line that wraps, so a long paragraph like this one takes a different number of lines at each width."
local paragraphs = table.create(10, PARAGRAPH)
local text = table.concat(paragraphs, "\n\n")

local view = ui.ref()
local offset = ui.state("markdown_window_offset", 0)
return ui.div {
	ref = view,
	id = "view",
	relative = true,
	w = 660,
	h = 400,
	overflow = "hidden",
	-- The same words, not windowed: how tall they stand.
	ui.div {
		absolute = true,
		id = "whole",
		left = 0,
		top = 500,
		w = 660,
		Markdown { text = text },
	},
	-- Far above the view (past OVERSCAN), the frame gone past the 8 ms
	-- the window judges by, and the offset written: the run is judged
	-- before its `on_resize` is heard.
	ui.div {
		absolute = true,
		id = "row",
		left = 0,
		top = -5000,
		w = 660,
		on_resize = function()
			local from = os.clock()
			while os.clock() - from < 0.012 do
				-- (The frame's own work.)
			end
			offset:set(offset:peek() + 1)
		end,
		Markdown {
			text = function(): string
				return text
			end,
			window = { key = "fixture:t", view = view, offset = offset },
		},
	},
}

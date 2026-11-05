-- A file viewer on a file made up here, for
-- tests/file_viewer_in_place.test.luau, in each of its header shapes
-- (#line, #stack, #path): the file open is `test_viewer_doc`, set to
-- another to open it. Here, beside app.luau, as a mounted script
-- loads modules from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local FileViewer = require("@ds/file_viewer")

local doc = ui.state("test_viewer_doc", {
	path = "src/ds/button.luau",
	state = "ready",
	text = "return 1\n",
	status = "modified",
} :: any)

-- Each header shape, one over another.
local function viewer(head: string): Node
	return ui.column {
		id = head,
		w_full = true,
		h = 130,
		FileViewer {
			doc = doc,
			root = "zeron",
			readonly = true,
			head = head :: any,
			w_full = true,
			h_full = true,
		},
	}
end

return ui.column {
	w_full = true,
	h_full = true,
	viewer("line"),
	viewer("stack"),
	viewer("path"),
}

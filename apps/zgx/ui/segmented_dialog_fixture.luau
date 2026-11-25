-- A dialog whose first control is in its scrolling content, as the MCP
-- server's and the project editor's are: a field, a filling choice, and one as
-- wide as its labels. What tests/segmented_dialog.test.luau measures.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()

local open = ui.state("seg_dialog_open", false)
local three = { { value = "a", label = "Default" }, { value = "b", label = "Emoji" }, { value = "c", label = "Image" } }

return ui.column {
	w = 900,
	h = 700,
	bg = "$background",
	ds.Dialog {
		open = open,
		title = "Choose",
		size = "md",
		confirm = { label = "Done" },
		ds.TextField { value = ui.state("seg_dialog_text", ""), label = "Name", w_full = true },
		ds.Segmented { id = "filled", value = ui.state("seg_dialog_filled", "a"), options = three, fill = true },
		ui.row { ds.Segmented { id = "labels", value = ui.state("seg_dialog_labels", "b"), options = three } },
	},
}

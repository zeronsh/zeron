-- Segmented controls in a column that stretches its children, at several
-- widths: filling (a dialog's three equal choices), fixed segments, and
-- segments as wide as their labels. What tests/segmented.test.luau measures:
-- the thumb's inset from its track, the same on every side.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()

local three = { { value = "a", label = "Default" }, { value = "b", label = "Emoji" }, { value = "c", label = "Image" } }
local two = { { value = "light", label = "Light" }, { value = "dark", label = "Dark" } }

local function stretched(w: number, child: Node): Node
	-- (A column stretches what's in it: the case that left room past the
	-- last segment.)
	return ui.column { w = w, items = "stretch", child }
end

return ui.column {
	w = 640,
	h = 520,
	p = 20,
	gap = 16,
	bg = "$background",
	stretched(410, ds.Segmented { id = "fill-410", value = ui.state("seg_fill_410", "c"), options = three, fill = true }),
	stretched(300, ds.Segmented { id = "fill-300", value = ui.state("seg_fill_300", "b"), options = three, fill = true }),
	stretched(600, ds.Segmented { id = "fill-600", value = ui.state("seg_fill_600", "a"), options = two, fill = true }),
	stretched(410, ds.Segmented { id = "fixed-410", value = ui.state("seg_fixed_410", "c"), options = three, segment = 96 }),
	stretched(410, ds.Segmented { id = "labels-410", value = ui.state("seg_labels_410", "dark"), options = two }),
}

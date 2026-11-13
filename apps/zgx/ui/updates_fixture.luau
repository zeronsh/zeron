-- The update button and its list, alone: the rows as src/updates.rs gives them.
ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Updates = require("@app/screens/shell/updates")

return ui.column {
	w_full = true,
	h_full = true,
	items = "end",
	p = "$space.3",
	ui.show(Updates.shown, Updates.Button {}),
	require("@ds/toast").Host {},
}

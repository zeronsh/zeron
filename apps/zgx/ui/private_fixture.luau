-- ds.PrivateText on its own, with no engine: three addresses in settings
-- rows and one in a line of text, what tests/private_text.test.luau drives.
-- Each address's stand-in is in state (`private_stand_ins`), so a test can
-- hold it to the rules (the same each time, separators kept).

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
-- Addresses plain to start (hiding is the app's default): the tests read
-- them, and private_text's turn hiding on themselves.
ds.PrivateText.hidden:set(false)

local EMAILS = { "mara@halcyon.dev", "theo.lindqvist@halcyon.dev", "ines_duarte+zeron@north-wind.io" }

local stand_ins = ui.state("private_stand_ins", {} :: { string })
local list = {}
for _, e in EMAILS do
	table.insert(list, ds.PrivateText.stand_in(e))
end
stand_ins:set(list)

ui.command {
	id = "private.hide",
	title = "Hide addresses",
	run = function()
		ds.PrivateText.hidden:set(true)
	end,
}
ui.command {
	id = "private.show",
	title = "Show addresses",
	run = function()
		ds.PrivateText.hidden:set(false)
	end,
}

local rows: { Node } = {}
for i, e in EMAILS do
	table.insert(rows, ds.Settings.Row { label = `Person {i}`, description = e, private_description = true })
end

return ui.column {
	id = "private",
	p = "$space.6",
	gap = "$space.4",
	w_full = true,
	h_full = true,
	ds.Settings.Group(rows :: any),
	ui.row {
		id = "private-line",
		gap = "$space.1",
		items = "baseline",
		ui.text { "Signed in as", text_size = "$font.size.md" },
		ds.PrivateText { "mara@halcyon.dev", text_size = "$font.size.md" },
	},
}

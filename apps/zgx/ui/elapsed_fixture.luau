-- Running timers at a few lengths (ds/locale `elapsed`), in the app's words.
local locale = require("@ds/locale")

return ui.column {
	ui.text { locale.elapsed(42), literal = true },
	ui.text { locale.elapsed(60), literal = true },
	ui.text { locale.elapsed(202), literal = true },
	ui.text { locale.elapsed(3922), literal = true },
}

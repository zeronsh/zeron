-- A date picker over a fixed March 2026, which starts on a Sunday, so its
-- days stand in its grid's first cells (`#dp-day-10` is March 10): chosen
-- the 10th at 10:00, nothing before the 5th at 9:20 to be had. What
-- tests/date_picker.test.luau presses on. (The moments are this Mac's
-- clock's, as the picker reads them.) `dp_demo` shows the gallery's page
-- instead (demos/date_picker.luau), to see that it builds.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local DatePicker = require("@ds/date_picker")

-- A day's minute on this Mac's clock. (Luau's os.time reads its table as
-- UTC: the offset os.date finds there puts it right, looked up twice for a
-- moment near a change to or from summer time.)
local function at(y: number, m: number, d: number, h: number, mi: number): number
	local wall = os.time({ year = y, month = m, day = d, hour = h, min = mi, sec = 0 })
	local function offset(t: number): number
		return os.time(os.date("*t", t) :: any) - t
	end
	return wall - offset(wall - offset(wall))
end

local value = ui.state("dp_value", at(2026, 3, 10, 10, 0) :: number?)
local changes = ui.state("dp_changes", 0)
local demo = ui.state("dp_demo", false)

return ui.column {
	p = "$space.6",
	gap = "$space.4",
	bg = "$background",
	ui.presence(demo, function(on: boolean): Node
		return if on then require("@demos/date_picker").demo() else ui.div {}
	end),
	DatePicker {
		id = "dp",
		value = value,
		min = at(2026, 3, 5, 9, 20),
		on_change = function()
			changes:set(changes:peek() + 1)
		end,
	},
}

-- Date picker: a day and its time, from today on; a day alone; nothing
-- chosen yet.

local ds = require("@ds")
local kit = require("@demos/kit")

-- Tomorrow at 9 on this Mac's clock, to start from.
local function tomorrow_at_nine(): number
	local d = os.date("*t", os.time()) :: any
	local wall = os.time({ year = d.year, month = d.month, day = d.day + 1, hour = 9, min = 0, sec = 0 })
	local function offset(t: number): number
		return os.time(os.date("*t", t) :: any) - t
	end
	return wall - offset(wall - offset(wall))
end

local when = ui.state("dpk_when", tomorrow_at_nine() :: number?)
local day = ui.state("dpk_day", tomorrow_at_nine() :: number?)
local empty = ui.state("dpk_empty", nil :: number?)

-- What a picker holds, in words.
local function says(value: State<number?>): Node
	return ds.Text {
		function(): string
			local v = value:get()
			return if v then ds.Locale.exact(v) else "Nothing chosen"
		end,
		style = "caption",
		tone = "tertiary",
	}
end

return {
	id = "date_picker",
	title = "Date picker",
	about = "A month of days and the time of day under it. Today's number is in the accent; the chosen day takes the menus' selected wash. Days and times before `min` can't be chosen. The arrows move between days, Page Up and Page Down a month.",
	demo = function()
		return kit.page(ui.row {
			gap = "$space.8",
			items = "start",
			flex_wrap = true,
			kit.block(
				"FROM NOW ON, WITH ITS TIME",
				ds.DatePicker {
					id = "dpk-when",
					value = when,
					min = function(): number
						return os.time()
					end,
				},
				says(when)
			),
			kit.block("A DAY ALONE", ds.DatePicker { id = "dpk-day", value = day, time = false }, says(day)),
			kit.block("NOTHING CHOSEN YET", ds.DatePicker { id = "dpk-empty", value = empty }, says(empty)),
		})
	end,
}

-- A date picker: a month of days to choose one from, and the time of day
-- under it, as wide as where it's put. One edge and one rhythm: the month's
-- name, the first day's box, the hairline and the time's label start on
-- its left edge; the next-month arrow, the last day's box and the time's
-- field end on its right; its parts stand one gap apart (`space.3`), the
-- weekdays a small one over their days. The days are boxes spread edge to
-- edge, a hairline between weeks (a sixth week only for a month that
-- reaches into it; other months' days left blank). Today wears a dot of
-- the accent under its number; the chosen day the segmented control's
-- selected wash, flat. Days before `min` read disabled and can't be
-- chosen, nor times before it on its day.
--
--   DatePicker { value = state, min = os.time(), on_change = function(at) … end }
--
-- `value` holds the moment chosen, Unix seconds, read in local time (nil:
-- nothing yet). Choosing a day keeps the time of day it holds; choosing a
-- time keeps its day. With nothing chosen, a day takes 9:00 (midnight with
-- `time = false`), and a time takes the first day that can be chosen. A
-- moment before `min` becomes `min` itself. `min` is a value, a state or a
-- function. `time = false` leaves out the time of day.
--
-- Keys: the arrows move from day to day (up and down a week), Home and End
-- to the week's ends, Page Up and Page Down a month; Enter or Space takes
-- the day. The days are one Tab stop.
--
-- Its six weeks are built once and change in place: another month renames
-- the days where they stand, and choosing a day restyles the two it moves
-- between, nothing more. It opens on the value's month (else `min`'s,
-- else this one). `id` names it, and its parts after it: `<id>-title`,
-- `<id>-previous`, `<id>-next`, `<id>-day-1` … `<id>-day-42` (from the
-- first week's first day) and `<id>-time`.

local IconButton = require("@self/icon_button").IconButton
local Select = require("@self/select")
local control = require("@self/control")
local locale = require("@self/locale")

export type Props = {
	value: State<number?>,
	min: control.Live<number?>?,
	-- Whether the time of day shows under the days (it does unless false).
	time: boolean?,
	on_change: ((at: number) -> ())?,
	id: string?,
	-- What assistive technology calls the days.
	label: Label?,
}

local DAY = 86400
-- A day's box (its wash): as wide as a weekday's short name needs, as tall
-- as a compact row.
local CELL_W, CELL_H = 36, 32
local WEEKS = 6
-- The times of day offered, minutes apart.
local STEP = 15
-- A day's time with nothing chosen yet.
local MORNING = 9 * 60
-- No day: before every day there is (nothing chosen, no `min`).
local NONE = -1e9

-- (Days are counted from 1970-01-01 on the calendar, by arithmetic: no
-- time zone, so a day is always one apart from the next. Howard Hinnant's
-- `days_from_civil` and `civil_from_days`.)
local function days(y: number, m: number, d: number): number
	-- (A month past either end of the year carries into the year.)
	y += (m - 1) // 12
	m = (m - 1) % 12 + 1
	if m <= 2 then
		y -= 1
	end
	local era = y // 400
	local yoe = y - era * 400
	local doy = (153 * (if m > 2 then m - 3 else m + 9) + 2) // 5 + d - 1
	local doe = yoe * 365 + yoe // 4 - yoe // 100 + doy
	return era * 146097 + doe - 719468
end

local function civil(z: number): (number, number, number)
	z += 719468
	local era = z // 146097
	local doe = z - era * 146097
	local yoe = (doe - doe // 1460 + doe // 36524 - doe // 146096) // 365
	local doy = doe - (365 * yoe + yoe // 4 - yoe // 100)
	local mp = (5 * doy + 2) // 153
	local d = doy - (153 * mp + 2) // 5 + 1
	local m = if mp < 10 then mp + 3 else mp - 9
	return yoe + era * 400 + (if m <= 2 then 1 else 0), m, d
end

-- A day's weekday, 1 (Sunday) to 7, as os.date and `time.weekday_short` count.
local function weekday(z: number): number
	return (z + 4) % 7 + 1
end

-- A moment's day and minute of the day, on this Mac's clock.
local function day_of(at: number): (number, number)
	local d = os.date("*t", math.floor(at)) :: any
	return days(d.year, d.month, d.day), d.hour * 60 + d.min
end

-- How far this Mac's clock is ahead of UTC at a moment, in seconds.
local function offset(at: number): number
	local d = os.date("*t", math.floor(at)) :: any
	return days(d.year, d.month, d.day) * DAY + d.hour * 3600 + d.min * 60 + d.sec - math.floor(at)
end

-- The moment a day's minute starts on this Mac's clock. (Twice: the
-- offset where it lands may differ from the guess's, across a change to
-- or from summer time.)
local function at(day: number, minute: number): number
	local wall = day * DAY + minute * 60
	local guess = wall - offset(wall)
	return wall - offset(guess)
end

-- A month as one number (year * 12 + month - 1), so the next is one on.
local function month_of(day: number): number
	local y, m = civil(day)
	return y * 12 + m - 1
end

local function first_of(month: number): number
	return days(month // 12, month % 12 + 1, 1)
end

-- A time of day's words, on the 12-hour clock ("9:15 AM").
local function clock(minute: number): string
	local h = (minute // 60) % 12
	return locale.s("time.clock_12", {
		h = tostring(if h == 0 then 12 else h),
		mm = string.format("%02d", minute % 60),
		half = if minute < 12 * 60 then "am" else "pm",
	})
end

return ui.component(function(props: Props): Node
	local value = props.value
	local base = props.id or "date-picker"
	local with_time = props.time ~= false

	-- The earliest moment that can be chosen (nil: any), and its day (NONE).
	local function min_at(): number?
		return control.value(props.min) :: number?
	end
	local min_day = ui.computed(function(): number
		local m = min_at()
		if m == nil then
			return NONE
		end
		local d = day_of(m)
		return d
	end)
	local today = day_of(os.time())
	local chosen = ui.computed(function(): number
		local v = value:get()
		if v == nil then
			return NONE
		end
		local d = day_of(v)
		return d
	end)

	-- The month showing.
	local start_month: number
	do
		local v, m = value:peek(), min_at()
		start_month = month_of(if v then day_of(v) elseif m then day_of(m) else today)
	end
	local month = ui.use_state(start_month)
	-- The weekday a week starts on (the locale's: Sunday in English).
	local first_weekday = ui.computed(function(): number
		local n = tonumber(locale.s("ds.date_picker.first_weekday"))
		return if n and n >= 1 and n <= 7 then n else 1
	end)
	-- The first day the six weeks show, for a month.
	local function grid_start(m: number): number
		local first = first_of(m)
		return first - (weekday(first) - first_weekday:peek()) % 7
	end
	local start = ui.computed(function(): number
		local first = first_of(month:get())
		return first - (weekday(first) - first_weekday:get()) % 7
	end)

	local function set(t: number)
		local m = min_at()
		if m and t < m then
			t = m
		end
		value:set(t)
		if props.on_change then
			props.on_change(t)
		end
	end
	-- A day taken: at the time of day the value holds.
	local function pick(day: number)
		if day < min_day:peek() then
			return
		end
		local v = value:peek()
		local minute = if v then select(2, day_of(v)) else (if with_time then MORNING else 0)
		set(at(day, minute))
		local m = month_of(day)
		if m ~= month:peek() then
			month:set(m)
		end
	end

	-- The days' elements, to move the keys' focus among them.
	local cells: { Element } = {}
	-- Focus on a day, showing its month first if it's another.
	local function go(day: number)
		day = math.max(day, min_day:peek())
		local m = month_of(day)
		local s = grid_start(month:peek())
		-- (Another month's day, here or past the grid: its month first.)
		if m ~= month:peek() then
			month:set(m)
			s = grid_start(m)
		end
		local el = cells[day - s + 1]
		if el then
			el:focus()
		end
	end
	-- The same day of the month `by` months on (the month's last, past it).
	local function months_on(day: number, by: number): number
		local y, m, d = civil(day)
		local target = y * 12 + m - 1 + by
		local last = first_of(target + 1) - first_of(target)
		return first_of(target) + math.min(d, last) - 1
	end

	local function cell(i: number): Node
		local function day(): number
			return start:get() + i - 1
		end
		local function outside(): boolean
			return month_of(day()) ~= month:get()
		end
		local function blocked(): boolean
			return day() < min_day:get()
		end
		local function on(): boolean
			return chosen:get() == day() and not outside()
		end
		return ui.div {
				on_mount = function(el: Element)
					cells[i] = el
				end,
				id = `{base}-day-{i}`,
				role = "gridcell",
				focusable = true,
				label = function(): string
					return if outside() then "" else locale.date(at(day(), 12 * 60))
				end,
				selected = on,
				disabled = function(): boolean
					return blocked() or outside()
				end,
				w = CELL_W,
				h = CELL_H,
				shrink = 0,
				relative = true,
				items = "center",
				justify = "center",
				rounded = "$radius.md",
				-- (Another month's day: nothing, and nothing to press.)
				opacity = function(): number
					return if outside() then 0 else 1
				end,
				pointer_events = function(): any
					return if outside() or blocked() then "none" else "auto"
				end,
				cursor = "pointer",
				bg = function(): Color
					return if on() then "$zeron.segmented.thumb" else "#00000000"
				end,
				hover = { bg = "$hover" },
				active = { bg = "$pressed" },
				focus = control.ring,
				on_click = function()
					if not outside() then
						pick(day())
					end
				end,
				on_key = function(e: KeyEvent): boolean
					local d = day()
					local k = e.key
					if k == "Enter" or k == " " then
						pick(d)
					elseif k == "ArrowLeft" then
						go(d - 1)
					elseif k == "ArrowRight" then
						go(d + 1)
					elseif k == "ArrowUp" then
						go(d - 7)
					elseif k == "ArrowDown" then
						go(d + 7)
					elseif k == "Home" then
						go(d - (weekday(d) - first_weekday:peek()) % 7)
					elseif k == "End" then
						go(d + 6 - (weekday(d) - first_weekday:peek()) % 7)
					elseif k == "PageUp" then
						go(months_on(d, -1))
					elseif k == "PageDown" then
						go(months_on(d, 1))
					else
						return false
					end
					return true
				end,
				ui.text {
					function(): string
						local _, _, d = civil(day())
						return tostring(d)
					end,
					text_size = "$font.size.md",
					tabular = true,
					weight = function(): any
						return if on() then "$font.weight.semibold" else "$font.weight.regular"
					end,
					color = function(): Color
						return if blocked() then "$text.disabled" else "$text.primary"
					end,
				},
				-- Today: a dot of the accent under its number.
				ui.div {
					absolute = true,
					bottom = 3,
					w = 4,
					h = 4,
					rounded = "$radius.full",
					bg = "$accent",
					opacity = function(): number
						return if day() == today and not outside() then 1 else 0
					end,
					aria_hidden = true,
				},
		}
	end

	local heads: { Node } = {}
	for i = 1, 7 do
		table.insert(
			heads,
			ui.text {
				function(): string
					return locale.s("time.weekday_short", { d = tostring((first_weekday:get() + i - 2) % 7 + 1) })
				end,
				w = CELL_W,
				shrink = 0,
				align = "center",
				text_size = "$font.size.xs",
				weight = "$font.weight.medium",
				color = "$text.tertiary",
				aria_hidden = true,
			}
		)
	end
	-- Its weeks: a sixth only for a month that reaches into it (no row of
	-- blanks under a month of five).
	local six = ui.computed(function(): boolean
		return first_of(month:get() + 1) > start:get() + 35
	end)
	local rows: { Node } = {}
	for week = 0, WEEKS - 1 do
		local row: { Node } = {}
		for d = 1, 7 do
			table.insert(row, cell(week * 7 + d))
		end
		local r = ui.row { w_full = true, justify = "between", role = "row", table.unpack(row) }
		table.insert(rows, if week == WEEKS - 1 then ui.show(six, r) else r)
	end

	-- The time of day: the value's own minute, read and written through it
	-- (one state: a Select's value shaped over the picker's).
	local function minute_now(read: () -> number?): number?
		local v = read()
		if v == nil then
			return nil
		end
		local _, minute = day_of(v)
		return minute
	end
	local time_of_day = {
		get = function(_self: any): number?
			return minute_now(function()
				return value:get()
			end)
		end,
		peek = function(_self: any): number?
			return minute_now(function()
				return value:peek()
			end)
		end,
		set = function(_self: any, minute: number)
			local v = value:peek()
			local day = if v then day_of(v) else math.max(today, min_day:peek())
			set(at(day, minute))
		end,
	}
	-- The value's minute when it falls between the steps (offered too, so
	-- the field shows it), else -1.
	local between = ui.computed(function(): number
		local minute = minute_now(function()
			return value:get()
		end)
		return if minute and minute % STEP ~= 0 then minute else -1
	end)
	-- The first minute the chosen day can take: `min`'s, on its day.
	local earliest = ui.computed(function(): number
		local m = min_at()
		if m == nil or chosen:get() ~= min_day:get() then
			return 0
		end
		local _, minute = day_of(m)
		return minute
	end)
	local times = ui.computed(function(): { any }
		local odd, floor = between:get(), earliest:get()
		local out = {}
		for minute = 0, 24 * 60 - STEP, STEP do
			table.insert(out, { value = minute, label = clock(minute), disabled = minute < floor })
			-- (The value's own minute after the step before it.)
			if odd > minute and odd < minute + STEP then
				table.insert(out, { value = odd, label = clock(odd), disabled = odd < floor })
			end
		end
		return out
	end)

	local before_min = function(): boolean
		return first_of(month:get()) <= min_day:get()
	end
	return ui.column {
		w_full = true,
		gap = "$space.3",
		-- The month, and its arrows at the right edge.
		ui.row {
			w_full = true,
			items = "center",
			ui.text {
				function(): string
					local m = month:get()
					return locale.s("ds.date_picker.month_year", { m = tostring(m % 12 + 1), year = tostring(m // 12) })
				end,
				id = `{base}-title`,
				grow = 1,
				min_w = 0,
				text_size = "$font.size.md",
				weight = "$font.weight.semibold",
				color = "$text.primary",
				live = "polite",
			},
			IconButton {
				id = `{base}-previous`,
				icon = "alt-arrow-left",
				size = "sm",
				label = locale.t("ds.date_picker.previous_month"),
				tooltip = false,
				disabled = before_min,
				on_click = function()
					if not before_min() then
						month:set(month:peek() - 1)
					end
				end,
			},
			IconButton {
				id = `{base}-next`,
				icon = "alt-arrow-right",
				size = "sm",
				label = locale.t("ds.date_picker.next_month"),
				tooltip = false,
				on_click = function()
					month:set(month:peek() + 1)
				end,
			},
		},
		-- The weekdays over their days.
		ui.column {
			w_full = true,
			gap = "$space.1",
			ui.row { w_full = true, justify = "between", aria_hidden = true, table.unpack(heads) },
			ui.column {
				w_full = true,
				gap = "$zeron.space.hair",
				role = "grid",
				label = props.label or locale.t("ds.date_picker.label"),
				focus_group = "both",
				table.unpack(rows),
			},
		},
		if with_time then ui.div { h = 1, w_full = true, shrink = 0, bg = "$border.subtle", aria_hidden = true } else nil,
		if with_time
			then ui.row {
				w_full = true,
				items = "center",
				gap = "$space.3",
				ui.text {
					locale.t("ds.date_picker.time"),
					grow = 1,
					min_w = 0,
					text_size = "$font.size.md",
					color = "$text.secondary",
				},
				ui.div {
					id = `{base}-time`,
					w = 132,
					shrink = 0,
					Select {
						value = time_of_day :: any,
						options = function(): { any }
							return times:get()
						end,
						name = locale.t("ds.date_picker.time"),
						size = "sm",
						w_full = true,
					},
				},
			}
			else nil,
	}
end)

-- The date picker (ds/date_picker) on ui/date_picker_fixture.luau: March
-- 2026, chosen the 10th at 10:00, nothing before the 5th at 9:20. March
-- 2026 starts on a Sunday, so day N stands in cell N.

local VALUE = "dp_value"
local CHANGES = "dp_changes"

local function open(): any
	local app = mount("date_picker_fixture.luau")
	app:resize(480, 800)
	app:frames(10)
	return app
end

-- The value's date and time on this Mac's clock: "2026-03-10 10:00".
local function chosen(app: any): string
	local v = app:state(VALUE)
	if v == nil then
		return "nothing"
	end
	return os.date("%Y-%m-%d %H:%M", v :: number) :: string
end

local function cell(app: any, i: number): string
	return app:text(`#dp-day-{i}`)
end

test("DatePicker: opens on the value's month, the chosen day where it falls", function()
	local app = open()
	expect(cell(app, 1)).to_be("1")
	expect(cell(app, 10)).to_be("10")
	expect(cell(app, 31)).to_be("31")
	-- (March fits five weeks: no sixth row of April's days.)
	expect(app:has("#dp-day-42")).to_be(false)
	expect(chosen(app)).to_be("2026-03-10 10:00")
	-- May 2026 starts on a Friday and runs into a sixth week.
	app:click("#dp-next")
	app:frames(10)
	app:click("#dp-next")
	app:frames(10)
	expect(app:text("#dp-title")).to_be("May 2026")
	expect(app:has("#dp-day-42")).to_be(true)
	expect(cell(app, 36)).to_be("31")
end)

test("DatePicker: picking a day keeps the time of day", function()
	local app = open()
	app:click("#dp-day-20")
	app:frames(2)
	expect(chosen(app)).to_be("2026-03-20 10:00")
	expect(app:state(CHANGES)).to_be(1)
end)

test("DatePicker: the next month renames the days in place, and a day there keeps the time", function()
	local app = open()
	app:click("#dp-next")
	app:frames(2)
	-- April 2026 starts on a Wednesday: March's last three days first.
	expect(cell(app, 1)).to_be("29")
	expect(cell(app, 4)).to_be("1")
	app:click("#dp-day-18")
	app:frames(2)
	expect(chosen(app)).to_be("2026-04-15 10:00")
	-- And back.
	app:click("#dp-previous")
	app:frames(2)
	expect(cell(app, 1)).to_be("1")
	expect(cell(app, 32)).to_be("1")
end)

test("DatePicker: a day before min can't be picked, nor a month before its own shown", function()
	local app = open()
	-- (A press on a day it won't take: failing to land, or landing on nothing.)
	pcall(function()
		app:click("#dp-day-3")
	end)
	app:frames(2)
	expect(chosen(app)).to_be("2026-03-10 10:00")
	expect(app:state(CHANGES)).to_be(0)
	pcall(function()
		app:click("#dp-previous")
	end)
	app:frames(2)
	expect(cell(app, 1)).to_be("1")
	expect(cell(app, 10)).to_be("10")
	-- min's own day can be: at the time it holds, past min's.
	app:click("#dp-day-5")
	app:frames(2)
	expect(chosen(app)).to_be("2026-03-05 10:00")
end)

test("DatePicker: a time changes the time of day, keeping the day", function()
	local app = open()
	app:click("#dp-time")
	app:frames(10)
	-- Every 15 minutes from midnight: 10:30 is the 43rd.
	app:click("#select-option-43")
	app:frames(10)
	expect(chosen(app)).to_be("2026-03-10 10:30")
end)

test("DatePicker: on min's day, the times before min can't be taken", function()
	local app = open()
	app:click("#dp-day-5")
	app:frames(2)
	app:click("#dp-time")
	app:frames(10)
	-- 9:15, before min's 9:20.
	pcall(function()
		app:click("#select-option-38")
	end)
	app:frames(10)
	expect(chosen(app)).to_be("2026-03-05 10:00")
end)

test("DatePicker: the arrows move between days, Page Down a month, Enter picks", function()
	local app = open()
	app:click("#dp-day-10")
	app:frames(2)
	app:key("ArrowRight")
	app:key("ArrowDown")
	app:frames(2)
	app:key("Enter")
	app:frames(2)
	expect(chosen(app)).to_be("2026-03-18 10:00")
	app:key("PageDown")
	app:frames(2)
	app:key("Enter")
	app:frames(2)
	expect(chosen(app)).to_be("2026-04-18 10:00")
	-- (April showing now.)
	expect(cell(app, 4)).to_be("1")
end)

-- A node's identity in the tree (`@n`), by its id.
local function node(app: any, id: string): string?
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(`#{id} `, 1, true) or line:find(`#{id}$`) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

test("DatePicker: the days stay the same nodes as a day is picked and the month changes", function()
	local app = open()
	local before = node(app, "dp-day-20")
	expect(before).never.to_be_nil()
	app:click("#dp-day-20")
	app:frames(2)
	expect(`day @{node(app, "dp-day-20")}`).to_be(`day @{before}`)
	app:click("#dp-next")
	app:frames(2)
	expect(`day @{node(app, "dp-day-20")}`).to_be(`day @{before}`)
end)

test("DatePicker: a time between the steps is offered where it falls", function()
	local app = open()
	local v = app:state(VALUE) :: number
	app:set_state(VALUE, v + 7 * 60)
	app:frames(2)
	app:click("#dp-time")
	app:frames(10)
	-- After 10:00 (the 41st), before 10:15.
	expect(app:text("#select-option-42")).to_contain("10:07")
	app:click("#select-option-44")
	app:frames(10)
	expect(chosen(app)).to_be("2026-03-10 10:30")
end)

test("DatePicker: the gallery's page builds its three pickers", function()
	local app = open()
	app:set_state("dp_demo", true)
	app:frames(10)
	expect(app:error()).to_be_nil()
	for _, id in { "dpk-when", "dpk-day", "dpk-empty" } do
		expect(app:has(`#{id}-day-1`)).to_be(true)
	end
	expect(app:has("#dpk-when-time")).to_be(true)
	expect(app:has("#dpk-day-time")).to_be(false)
end)

-- A segmented control's thumb sits the same inset from its track on every
-- side of the segment it marks: never short of the track's end, never past
-- it, at every width, filling or not (ui/segmented_fixture.luau, whose
-- column stretches its children). (Its corner is the track's less that
-- inset: ds/segmented derives it with control.nest.)

local INSET = 2

-- A node's box from the tree: x, y, w, h.
local function box(app: any, id: string): (number, number, number, number)
	for line in app:tree(`#{id}`):gmatch("[^\n]+") do
		if line:find(`#{id} `, 1, true) or line:find(`#{id}$`) then
			local x, y, w, h = line:match(" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
			if x then
				return tonumber(x) :: number, tonumber(y) :: number, tonumber(w) :: number, tonumber(h) :: number
			end
		end
	end
	error(`no #{id} in the tree:\n{app:tree()}`)
end

local function near(a: number, b: number): boolean
	return math.abs(a - b) <= 0.5
end

-- The thumb, settled, against its track: which segment it marks (1-based)
-- of `n`, each an equal share when `equal`.
local function check(app: any, id: string, chosen: number, n: number, equal: boolean)
	local tx, ty, tw, th = box(app, id)
	local x, y, w, h = box(app, `{id}-thumb`)
	local top, bottom = y - ty, (ty + th) - (y + h)
	expect(`{id} top {top}`).to_be(`{id} top {INSET}`)
	expect(`{id} bottom {bottom}`).to_be(`{id} bottom {INSET}`)
	if chosen == 1 then
		expect(`{id} left {near(x - tx, INSET)}`).to_be(`{id} left true`)
	end
	if chosen == n then
		expect(`{id} right {near((tx + tw) - (x + w), INSET)} ({(tx + tw) - (x + w)})`).to_be(`{id} right true ({(tx + tw) - (x + w)})`)
	end
	if equal then
		local share = (tw - INSET * 2) / n
		expect(`{id} share {near(w, share)} ({w} of {share})`).to_be(`{id} share true ({w} of {share})`)
		expect(`{id} at {near(x - tx, INSET + share * (chosen - 1))}`).to_be(`{id} at true`)
	end
end

local function open(): any
	local app = mount("segmented_fixture.luau")
	app:resize(700, 560)
	app:frames(90)
	return app
end

test("segmented: a filling track's thumb fills its share, the same inset all round, at every width", function()
	local app = open()
	check(app, "fill-410", 3, 3, true)
	check(app, "fill-300", 2, 3, true)
	check(app, "fill-600", 1, 2, true)
	-- Filling: the track is the column's whole width.
	local _, _, w = box(app, "fill-410")
	expect(w).to_be(410)
end)

test("segmented: a track never stretches past its segments, so its last thumb's inset is the same", function()
	local app = open()
	check(app, "fixed-410", 3, 3, true)
	local _, _, w = box(app, "fixed-410")
	expect(w).to_be(96 * 3 + INSET * 2)
	check(app, "labels-410", 2, 2, false)
	local _, _, lw = box(app, "labels-410")
	expect(lw < 410).to_be(true)
end)

test("segmented: choosing another segment moves the thumb there, inset as ever", function()
	local app = open()
	app:click(`#fill-410 >> role:radio "Default"`)
	app:frames(90)
	check(app, "fill-410", 1, 3, true)
	app:click(`#fill-410 >> role:radio "Emoji"`)
	app:frames(90)
	check(app, "fill-410", 2, 3, true)
end)

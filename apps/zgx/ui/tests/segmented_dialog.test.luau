-- A segmented control in a dialog whose opening focus is in its scrolling
-- content (ui/segmented_dialog_fixture.luau): the thumb stands at its place
-- from the first frame it's laid out, never sliding there from the width the
-- scroll had first; and it still moves as another is chosen, by the pointer or
-- the keys, at once with motion reduced.

-- A node's box from the tree, or nil while it isn't there.
local function box(app: any, id: string): string?
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(` #{id} `, 1, true) then
			return (line:match(" (%-?[%d.]+),%-?[%d.]+ [%d.]+x[%d.]+") or "") .. " " .. (line:match("([%d.]+x[%d.]+)") or "")
		end
	end
	return nil
end

local function mounted(reduced: boolean): any
	local app = mount("segmented_dialog_fixture.luau")
	app:resize(900, 700)
	app:frames(30)
	app:set_state("motion_reduce", reduced)
	app:frames(2)
	app:set_state("seg_dialog_open", true)
	return app
end

-- Where the thumb is along its track, and how wide.
local function thumb(app: any, id: string): (number, number)
	local at, size = (box(app, `{id}-thumb`) or ""):match("(%S+) (%S+)")
	local from = (box(app, id) or ""):match("(%S+) ")
	return (tonumber(at) :: number) - (tonumber(from) :: number), tonumber(((size :: string):match("^[%d.]+"))) :: number
end

for _, reduced in { false, true } do
	local said = if reduced then ", motion reduced" else ""
	test(`segmented in a dialog: it opens with the thumb in its place, every frame{said}`, function()
		for _, id in { "filled", "labels" } do
			local app = mounted(reduced)
			local widths = {}
			for _ = 1, 40 do
				app:frames(1)
				local x, w = thumb(app, id)
				widths[`{x} {w}`] = true
			end
			local n = 0
			for _ in widths do
				n += 1
			end
			expect(`{id}: {n}`).to_be(`{id}: 1`)
			expect(app:error()).to_be_nil()
		end
	end)

	test(`segmented in a dialog: the keys choose another, and the thumb goes there{said}`, function()
		local app = mounted(reduced)
		app:frames(60)
		local from, width = thumb(app, "filled")
		-- (The field has the focus as it opens; then the choice, its arrows
		-- choosing.)
		app:key("Tab")
		app:key("ArrowRight")
		expect(app:state("seg_dialog_filled")).to_be("b")
		app:frames(2)
		local after = thumb(app, "filled")
		app:frames(90)
		local to, landed = thumb(app, "filled")
		expect(math.abs(to - (from + width)) <= 1).to_be(true)
		expect(landed).to_be(width)
		if reduced then
			expect(after).to_be(to)
		else
			expect(after > from and after < to).to_be(true)
		end
		-- On again before it lands: it ends where the last one is.
		app:key("ArrowLeft")
		app:frames(2)
		app:key("ArrowRight")
		app:frames(2)
		app:key("ArrowRight")
		app:frames(90)
		expect(app:state("seg_dialog_filled")).to_be("c")
		expect(math.abs((thumb(app, "filled")) - (from + width * 2)) <= 1).to_be(true)
		expect(app:error()).to_be_nil()
	end)
end

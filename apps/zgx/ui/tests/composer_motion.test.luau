local Layout = require("@ds/composer_layout")
local Motion = require("@ds/composer_motion")

local function geometry(height: number, header: number?, corner: number?)
	return Layout.calculate({
		width = 660, sample = { unwrapped_width = 40, wrapped_height = height, line_height = 22 },
		trailing_width = 64, control_corner = corner or 25,
		header_height = header or 0, header_gap = 12,
		expanded = false, cargo = (header or 0) > 0, asking = false, dictating = false,
		multiline = height > 22,
	}, false)
end

test("composer transition: width changes once after a full expanded line fits", function()
	local a, b = geometry(22), geometry(220)
	local changes, last = 0, a.text.width
	local previous = Motion.frame(a, b, 0)
	for i = 0, 100 do
		local g = Motion.frame(a, b, i / 100)
		expect(g.text.width == a.text.width or g.text.width == b.text.width).to_be(true)
		if g.text.width ~= last then
			changes += 1
			-- A discrete width change must not also jump the vertical field.
			expect(math.abs(g.text.top - previous.text.top) < 1).to_be(true)
			expect(math.abs(g.text.height - previous.text.height) < 5).to_be(true)
		end
		last = g.text.width
		previous = g
		expect(g.text.height >= 22).to_be(true)
		expect(g.text.top >= 0).to_be(true)
		expect(g.text.top + g.text.height <= g.height - Layout.BORDER * 2 + 0.001).to_be(true)
		if g.expanded then
			expect(g.text.top + g.text.height < g.height - g.controls.trailing.bottom - g.controls.trailing.height).to_be(true)
		end
	end
	expect(changes).to_be(1)
	expect(Motion.frame(a, b, 1)).to_equal(b)
end)

test("composer transition: headers and rounder controls remain inside a valid frame", function()
	local a, b = geometry(22), geometry(500, 90, 45)
	for _, pair in { { a, b }, { b, a } } do
		for i = 0, 100 do
			local g = Motion.frame(pair[1], pair[2], i / 100)
			expect(g.header_height <= g.text.top + 0.001).to_be(true)
			expect(g.text.height >= 22).to_be(true)
			expect(g.text.top + g.text.height <= g.height - 2 + 0.001).to_be(true)
			if not g.expanded then
				expect(g.controls.leading.left + 28 < g.text.left).to_be(true)
			end
		end
	end
end)

test("composer transition: interrupted height and anchors start at the presented geometry", function()
	local shown = Motion.frame(geometry(22), geometry(500, 90), 0.3)
	local g = Motion.frame(shown, geometry(100, 30), 0)
	expect(g.height).to_be(shown.height)
	expect(g.header_height).to_be(shown.header_height)
	expect(g.controls.leading.bottom).to_be(shown.controls.leading.bottom)
	expect(g.controls.trailing.bottom).to_be(shown.controls.trailing.bottom)
end)

test("composer transition: changes above the cap do not change the visual destination", function()
	local a, b = geometry(500), geometry(900)
	expect(a.natural_height == b.natural_height).to_be(false)
	expect(Motion.same_destination(a, b)).to_be(true)
	local constraints = table.clone(a)
	constraints.line_height = 24
	constraints.compact_text = { left = 45.5, right = 100 }
	expect(Motion.same_destination(a, constraints)).to_be(true)
	expect(Motion.same_destination(a, geometry(100))).to_be(false)
	expect(Motion.same_destination(a, geometry(500, 30))).to_be(false)
end)

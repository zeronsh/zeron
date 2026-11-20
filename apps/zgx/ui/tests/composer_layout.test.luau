local Layout = require("@ds/composer_layout")

local function input(overrides: { [string]: any }?): Layout.Input
	local p = {
		width = 660,
		sample = { unwrapped_width = 0, wrapped_height = 22, line_height = 22 },
		trailing_width = 64,
		control_corner = 25,
		header_height = 0,
		header_gap = 12,
		expanded = false,
		cargo = false,
		asking = false,
		dictating = false,
		multiline = false,
	} :: any
	for k, v in overrides or {} do
		p[k] = v
	end
	return p
end

test("composer geometry: compact destination and measured controls", function()
	local g = Layout.calculate(input(), false)
	expect(g.mode).to_be("compact")
	expect(g.height).to_be(49)
	expect(g.natural_height).to_be(49)
	expect(g.text).to_equal({ left = 45.5, right = 81.5, top = 12.5, width = 531, height = 22 })
	expect(g.controls.leading).to_equal({ left = 9.5, bottom = 9.5, height = 28 })
	expect(g.controls.trailing).to_equal({ right = 9.5, bottom = 7.5, height = 32 })
	local more_controls = Layout.calculate(input({ trailing_width = 100 }), false)
	expect(more_controls.text.width).to_be(495)
	expect(more_controls.text.left).to_be(g.text.left)
	local unknown_controls = Layout.calculate(input({ trailing_width = 0 }), false)
	expect(unknown_controls).to_equal(g)
end)

test("composer geometry: expansion and contraction have distinct exact boundaries", function()
	local p = input()
	p.sample = { unwrapped_width = 531, wrapped_height = 22, line_height = 22 }
	expect(Layout.calculate(p, false).expanded).to_be(false)
	p.sample.unwrapped_width = 531.01
	local opened = Layout.calculate(p, false)
	expect(opened.expanded).to_be(true)
	expect(opened.next_text_expanded).to_be(true)
	p.sample.unwrapped_width = 500
	expect(Layout.calculate(p, opened.next_text_expanded).expanded).to_be(true)
	expect(Layout.calculate(p, false).expanded).to_be(false)
	p.sample.unwrapped_width = 499
	local closed = Layout.calculate(p, true)
	expect(closed.expanded).to_be(false)
	expect(closed.next_text_expanded).to_be(false)
end)

test("composer geometry: forced modes preserve explicit text history", function()
	for _, flag in { "expanded", "cargo", "asking", "dictating" } do
		local p = input({ [flag] = true })
		local g = Layout.calculate(p, false)
		expect(g.expanded).to_be(true)
		expect(g.next_text_expanded).to_be(false)
		;(p :: any)[flag] = false
		expect(Layout.calculate(p, g.next_text_expanded).mode).to_be("compact")
	end
	local lines = Layout.calculate(input({ multiline = true }), false)
	expect(lines.expanded).to_be(true)
	expect(lines.next_text_expanded).to_be(true)
end)

test("composer geometry: missing measurements and unknown width are explicit", function()
	local p = input()
	p.sample = nil
	expect(Layout.calculate(p, false).mode).to_be("compact")
	expect(Layout.calculate(p, true).mode).to_be("expanded")
	p.width = 0
	local unknown = Layout.calculate(p, true)
	expect(unknown.mode).to_be("compact")
	expect(unknown.next_text_expanded).to_be(true)
	expect(unknown.text.width).to_be(0)
	p.expanded = true
	expect(Layout.calculate(p, false).mode).to_be("expanded")
	expect(Layout.measurement(0)).to_equal({ left = 16, width = 0 })
	expect(Layout.measurement(660)).to_equal({ left = 16, width = 626 })
end)

test("composer geometry: natural height is distinct from minimum and maximum", function()
	local p = input({ expanded = true })
	local small = Layout.calculate(p, false)
	expect(small.height).to_be(120)
	expect(small.natural_height < small.height).to_be(true)
	expect(small.text.height).to_be(56)
	p.sample = { unwrapped_width = 40, wrapped_height = 500, line_height = 22 }
	local capped = Layout.calculate(p, false)
	expect(capped.height).to_be(304)
	expect(capped.text.height).to_be(240)
	expect(capped.natural_height > capped.height).to_be(true)
	p.sample.wrapped_height = 700
	local more = Layout.calculate(p, false)
	expect(more.height).to_be(capped.height)
	expect(more.text).to_equal(capped.text)
	expect(more.natural_height > capped.natural_height).to_be(true)
end)

test("composer geometry: cargo and questions place text below the measured header", function()
	for _, flag in { "cargo", "asking" } do
		local p = input({ [flag] = true, header_height = 90 })
		local g = Layout.calculate(p, false)
		expect(g.header_height).to_be(90)
		expect(g.text.top).to_be(102)
		expect(g.height).to_be(206)
		expect(g.text.height).to_be(56)
		expect(g.controls.leading.bottom).to_be(11)
		p.header_height = 30
		local smaller = Layout.calculate(p, false)
		expect(smaller.text.top).to_be(42)
		expect(smaller.height).to_be(146)
		p.header_gap = 20
		local spaced = Layout.calculate(p, false)
		expect(spaced.text.top - smaller.text.top).to_be(8)
		expect(spaced.height - smaller.height).to_be(8)
	end
	local stale = Layout.calculate(input({ expanded = true, header_height = 90 }), false)
	expect(stale.header_height).to_be(0)
	expect(stale.text.top).to_be(16)
	expect(stale.height).to_be(120)
end)

test("composer geometry: resolved theme corners keep controls and action space aligned", function()
	local normal = Layout.calculate(input({ expanded = true }), false)
	local rounder = Layout.calculate(input({ expanded = true, control_corner = 35 }), false)
	expect(rounder.controls.leading.bottom).to_be(21)
	expect(rounder.controls.trailing.bottom).to_be(19)
	expect(rounder.controls.voice.bottom).to_be(21)
	expect(rounder.height).to_be(130)
	expect(rounder.text).to_equal(normal.text)
	expect(rounder.tray_inset).to_be(22)
end)

test("composer geometry: calculation is deterministic and leaves inputs untouched", function()
	local p = input({ cargo = true, header_height = 90 })
	table.freeze(p.sample :: any)
	table.freeze(p)
	local a = Layout.calculate(p, false)
	local b = Layout.calculate(p, false)
	expect(a).to_equal(b)
	expect(p.cargo).to_be(true)
	expect(p.header_height).to_be(90)
	-- Mutating a returned rectangle cannot change a later calculation.
	a.text.top = -100
	expect(Layout.calculate(p, false)).to_equal(b)
end)

test("composer geometry: a draft past the cap is estimated, and lays out as measured", function()
	-- Fewer hard lines than fill the box: measured. One more newline each.
	expect(Layout.estimate("", 22)).to_be(nil)
	expect(Layout.estimate(string.rep("line\n", 9), 22)).to_be(nil)
	local est = Layout.estimate(string.rep("line\n", 5000), 22) :: Layout.Sample
	expect(est).never.to_be(nil)
	-- The same estimate however long: a keystroke past the cap changes nothing.
	expect(Layout.estimate(string.rep("line\n", 12), 22)).to_equal(est)
	-- Laid out as the exact measure of the long draft would be: open, capped.
	local estimated = Layout.calculate(input({ sample = est, multiline = true }), false)
	local measured = Layout.calculate(input({
		sample = { unwrapped_width = 900, wrapped_height = 5000 * 22, line_height = 22 },
		multiline = true,
	}), false)
	expect(estimated.height).to_be(measured.height)
	expect(Layout.same_target(estimated, measured)).to_be(true)
	-- A smaller line height needs more lines to reach the cap.
	expect(Layout.estimate(string.rep("line\n", 12), 14)).to_be(nil)
end)

test("composer geometry: a target differs only where a view or the motion reads it", function()
	local a = Layout.calculate(input(), false)
	-- Wider: the widths layout applies itself, nothing bound.
	expect(Layout.same_target(a, Layout.calculate(input({ width = 900 }), false))).to_be(true)
	expect(Layout.same_target(a, Layout.calculate(input({ trailing_width = 100 }), false))).to_be(false)
	expect(Layout.same_target(a, Layout.calculate(input({ expanded = true }), false))).to_be(false)
	expect(Layout.same_target(a, Layout.calculate(input({ control_corner = 30 }), false))).to_be(false)
end)

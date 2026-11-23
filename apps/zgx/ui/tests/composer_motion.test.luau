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

test("composer transition: width changes once while horizontal insets glide smoothly", function()
	local a, b = geometry(22), geometry(220)
	local changes, last = 0, a.text.width
	local previous = Motion.frame(a, b, 0)
	for i = 0, 100 do
		local g = Motion.frame(a, b, i / 100)
		expect(g.text.width == a.text.width or g.text.width == b.text.width).to_be(true)
		if g.text.width ~= last then
			changes += 1
		end
		-- Every presented edge (left, right, top, height) moves smoothly without teleports.
		expect(math.abs(g.text.left - previous.text.left) < 2).to_be(true)
		expect(math.abs(g.text.right - previous.text.right) < 2).to_be(true)
		expect(math.abs(g.text.top - previous.text.top) < 1).to_be(true)
		expect(math.abs(g.text.height - previous.text.height) < 5).to_be(true)
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

test("composer transition: headers and rounder controls remain inside a valid frame without overlapping text", function()
	local a, b = geometry(22), geometry(500, 90, 45)
	for _, pair in { { a, b }, { b, a } } do
		local from, to = pair[1], pair[2]
		local prev = Motion.frame(from, to, 0)
		for i = 0, 100 do
			local g = Motion.frame(from, to, i / 100)
			expect(g.header_height <= g.text.top + 0.001).to_be(true)
			expect(g.text.height >= 22).to_be(true)
			expect(g.text.top + g.text.height <= g.height - 2 + 0.001).to_be(true)
			-- Insets and leading control glide smoothly with no abrupt jump.
			expect(math.abs(g.text.left - prev.text.left) < 2).to_be(true)
			expect(math.abs(g.controls.leading.left - prev.controls.leading.left) < 2).to_be(true)
			-- Visible text never overlaps the leading `+` button in 2D:
			-- either it sits vertically above `+` or horizontally to its right.
			local inner_h = g.height - Layout.BORDER * 2
			local leading_top = inner_h - g.controls.leading.bottom - g.controls.leading.height
			local check_h = if to.expanded then Motion.clip_height(g, to) else to.text.height
			if g.text.top + check_h > leading_top + 0.01 then
				expect(g.controls.leading.left + 28 < g.text.left).to_be(true)
			end
			prev = g
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

test("composer transition: editor height holds room during growth and settles to the visible clip", function()
	local compact = geometry(22)
	local three = geometry(66)
	local six = geometry(132)
	local capped = geometry(500)

	-- Settled: matches visible text clip exactly.
	expect(Motion.editor_height(compact, compact, { unwrapped_width = 40, wrapped_height = 22, line_height = 22 })).to_be(22)
	expect(Motion.editor_height(three, three, { unwrapped_width = 40, wrapped_height = 66, line_height = 22 })).to_be(66)
	expect(Motion.editor_height(capped, capped, { unwrapped_width = 40, wrapped_height = 500, line_height = 22 })).to_be(240)

	-- Growing (expanded 3 -> 6): holds destination height from progress 0 until settled.
	local growing = Motion.frame(three, six, 0.25)
	expect(Motion.editor_height(growing, six, { unwrapped_width = 40, wrapped_height = 132, line_height = 22 })).to_be(132)

	-- Shrinking (expanded 6 -> 3): tracks shown height down to destination.
	local shrinking = Motion.frame(six, three, 0.25)
	expect(Motion.editor_height(shrinking, three, { unwrapped_width = 40, wrapped_height = 66, line_height = 22 })).to_be(shrinking.text.height)

	-- Frame-0 pending before target updates: holds pending height immediately.
	expect(Motion.editor_height(three, three, { unwrapped_width = 40, wrapped_height = 110, line_height = 22 })).to_be(110)

	-- Compact -> expanded (compact-width phase): accounts for extra wraps at compact width.
	local opening = Motion.frame(compact, six, 0.1)
	expect(opening.expanded).to_be(false)
	expect(Motion.editor_height(opening, six, { unwrapped_width = 600, wrapped_height = 132, line_height = 22 }) > six.text.height).to_be(true)
	-- Once width expands (p = 0.9), drops back to destination height.
	local opened = Motion.frame(compact, six, 0.9)
	expect(opened.expanded).to_be(true)
	expect(Motion.editor_height(opened, six, { unwrapped_width = 600, wrapped_height = 132, line_height = 22 })).to_be(six.text.height)

	-- Width-only resize while compact + typing across narrower compact room uses live_width.
	-- Old compact width is 531; live_width = 480 gives compact_w = 351.
	expect(Motion.editor_height(
		compact,
		compact,
		{ unwrapped_width = 360, wrapped_height = 22, line_height = 22 },
		480
	) >= Layout.TEXT_MIN_VIEWPORT).to_be(true)

	-- Capped draft opening from compact caps at TEXT_MAX_VIEWPORT (scrollable by design).
	local opening_cap = Motion.frame(compact, capped, 0.1)
	expect(Motion.editor_height(opening_cap, capped, { unwrapped_width = 600, wrapped_height = 500, line_height = 22 })).to_be(240)
end)

test("composer transition: clip height opens to the pill bottom during compact-width expansion", function()
	local compact = geometry(22)
	local two_lines = geometry(44)
	local six = geometry(132)

	-- Settled frames clip to shown.text.height exactly.
	expect(Motion.clip_height(compact, compact)).to_be(compact.text.height)
	expect(Motion.clip_height(two_lines, two_lines)).to_be(two_lines.text.height)
	expect(Motion.clip_height(six, six)).to_be(six.text.height)

	-- Compact -> expanded at p = 0.2 (16ms): controls are still beside the text,
	-- so clip_height opens past shown.text.height to hold line 2 and its caret.
	local frame1 = Motion.frame(compact, two_lines, 0.2)
	expect(frame1.expanded).to_be(false)
	expect(frame1.text.height < 44).to_be(true)
	expect(Motion.clip_height(frame1, two_lines) >= 44).to_be(true)
	expect(Motion.clip_height(frame1, two_lines) <= two_lines.text.height).to_be(true)

	-- Once expanded width takes over, clip_height matches shown.text.height (above controls).
	local opened = Motion.frame(compact, two_lines, 0.9)
	expect(opened.expanded).to_be(true)
	expect(Motion.clip_height(opened, two_lines)).to_be(opened.text.height)
end)

test("composer transition: editor width holds destination width during inset glide and defers to layout settled", function()
	local compact = geometry(22)
	local two_lines = geometry(44)
	local six = geometry(132)

	-- Settled or same-mode transitions return nil so the textarea uses "100%".
	expect(Motion.editor_width(compact, compact, 660)).to_be_nil()
	expect(Motion.editor_width(two_lines, two_lines, 660)).to_be_nil()
	expect(Motion.editor_width(Motion.frame(two_lines, six, 0.5), six, 660)).to_be_nil()

	-- Compact -> expanded holds the expanded destination width across the entire glide.
	for _, p in { 0, 0.2, 0.5, 0.8 } do
		local g = Motion.frame(compact, two_lines, p)
		expect(Motion.editor_width(g, two_lines, 660)).to_be(two_lines.text.width)
		-- Respects live pill width if the window resizes mid-transition.
		expect(Motion.editor_width(g, two_lines, 500)).to_be(500 - Layout.BORDER * 2 - two_lines.text.left - two_lines.text.right)
	end
	expect(Motion.editor_width(Motion.frame(compact, two_lines, 1), two_lines, 660)).to_be_nil()

	-- Expanded -> compact holds the compact destination width across the glide.
	local shrinking = Motion.frame(two_lines, compact, 0.4)
	expect(Motion.editor_width(shrinking, compact, 660)).to_be(compact.text.width)
end)

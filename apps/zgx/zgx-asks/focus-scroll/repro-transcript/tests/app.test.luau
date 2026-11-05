-- ⇧Tab from the composer up through the replies' tool rows: each focused
-- row comes into view clear of `scroll_padding.top` (the list's top 38 +
-- 32 = 70). On zgx 2e22d99d3 step 6 rests at y -255, above the view, no
-- script writing the offset meanwhile. Each frame of each step is logged.
local function fy(app: any): number?
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		if line:find(" focused", 1, true) then
			return tonumber(line:match(" %-?%d+,(%-?%d+) "))
		end
	end
	return nil
end

test("⇧Tab brings each reply's tool row into view, clear of the top padding", function()
	local app = mount("app.luau")
	app:resize(960, 700)
	app:frames(40)
	expect(app:error()).to_be(nil)
	app:click("#composer")
	app:frames(5)
	for step = 1, 6 do
		print(`step {step}: before, offset {app:state("transcript_offset")}, focused y {fy(app)}`)
		app:key("shift+Tab")
		for frame = 1, 9 do
			app:frames(1)
			print(`  frame {frame}: offset {app:state("transcript_offset")}, focused y {fy(app)}`)
		end
		local y = fy(app)
		expect(`step {step}: y {y}, clear {y ~= nil and y >= 70}`).to_be(`step {step}: y {y}, clear true`)
	end
end)
